use hodl_config::TokenSpec;
use hodl_core::error::{Error, Result};
use hodl_core::{
    Address, Amount, Chain, ChainId, FeeRate, PrivateKeyBytes, SendParams, SignedTx, TxId, TxRef,
    UnsignedTx,
};
use tracing::debug;

use crate::address as eth_address;
use crate::derive;
use crate::erc20;
use crate::network::NetworkParams;
use crate::rpc::EthRpcClient;
use crate::tx::{Eip1559Tx, sign};

/// Ethereum chain implementation (mainnet; BSC reuses this in M5).
pub struct EthereumChain {
    params: NetworkParams,
    rpc: EthRpcClient,
}

impl EthereumChain {
    pub fn new(params: NetworkParams, rpc: EthRpcClient) -> Self {
        Self { params, rpc }
    }

    /// Walk the account axis at `m/44'/60'/N'/0/0` from N=0 until `gap_limit`
    /// consecutive empty accounts. Calls `on_account` for each non-empty account
    /// (and unconditionally for account 0). "Empty" = native balance == 0;
    /// token balances are advisory and do not influence the gap walk.
    ///
    /// Account 0 is always reported even if empty — the user should always see
    /// their primary account rather than a confusing "no accounts found" state.
    ///
    /// Serial — one `eth_getBalance` + N `eth_call` per account. No parallelism.
    ///
    /// On a per-account RPC failure the error propagates immediately (the caller
    /// can surface it and stop). Per-token errors are non-fatal (returned as
    /// `Err(…)` in the per-token result vec) consistent with `token_balances`.
    #[allow(clippy::type_complexity)]
    pub fn scan_accounts_streaming(
        &self,
        seed: &[u8; 64],
        gap_limit: u32,
        tokens: &[TokenSpec],
        on_account: &mut dyn FnMut(u32, String, u128, Vec<(TokenSpec, Result<u128>)>),
    ) -> Result<()> {
        let mut empty_run = 0u32;
        let mut account = 0u32;

        loop {
            let addr_str = derive::derive_address(seed, account, 0)?;
            debug!("eth scan account={account} address={addr_str}");

            let native = self.rpc.eth_get_balance(&addr_str)?;

            let is_empty = native == 0;

            if is_empty {
                // Account 0 is always reported regardless.
                if account == 0 {
                    let token_results = if tokens.is_empty() {
                        Vec::new()
                    } else {
                        let holder = Address::new(addr_str.clone(), self.params.chain_id);
                        self.token_balances(&holder, tokens)
                    };
                    on_account(account, addr_str, native, token_results);
                }

                empty_run += 1;
                if empty_run >= gap_limit {
                    break;
                }
            } else {
                empty_run = 0;
                let holder = Address::new(addr_str.clone(), self.params.chain_id);
                let token_results = if tokens.is_empty() {
                    Vec::new()
                } else {
                    self.token_balances(&holder, tokens)
                };
                on_account(account, addr_str, native, token_results);
            }

            account += 1;
        }

        Ok(())
    }

    /// Read ERC-20 `balanceOf` for `holder` against each supplied token contract.
    ///
    /// Serial — one RPC call per token. RPC providers (Infura free tier, etc.)
    /// commonly throttle parallel calls; serial keeps failure modes predictable.
    ///
    /// Per-token errors are returned as `Err(...)` for that entry but do not
    /// abort the batch. Callers decide whether to skip / display / retry.
    pub fn token_balances(
        &self,
        holder: &Address,
        tokens: &[TokenSpec],
    ) -> Vec<(TokenSpec, Result<u128>)> {
        tokens
            .iter()
            .map(|token| {
                let result = erc20::encode_balance_of(holder.as_str())
                    .map_err(|e| Error::Chain(format!("encode balanceOf: {e}")))
                    .and_then(|calldata| self.rpc.eth_call(&token.address, &calldata))
                    .and_then(|bytes| {
                        erc20::decode_uint256_to_u128(&bytes)
                            .map_err(|e| Error::Chain(format!("decode balanceOf: {e}")))
                    });
                (token.clone(), result)
            })
            .collect()
    }
}

impl Chain for EthereumChain {
    fn id(&self) -> ChainId {
        self.params.chain_id
    }

    fn slip44(&self) -> u32 {
        self.params.chain_id.slip44()
    }

    fn derive(&self, seed: &[u8; 64], account: u32, index: u32) -> Result<Address> {
        let addr_str = derive::derive_address(seed, account, index)?;
        Ok(Address::new(addr_str, self.params.chain_id))
    }

    fn balance(&self, addr: &Address) -> Result<Amount> {
        let wei = self.rpc.eth_get_balance(addr.as_str())?;
        Ok(Amount::from_atoms(wei, self.params.chain_id))
    }

    fn history(&self, _addr: &Address) -> Result<Vec<TxRef>> {
        // Ethereum has no native address-indexed tx history in the JSON-RPC
        // surface — an external indexer (Alchemy, Etherscan, etc.) is required.
        tracing::debug!("eth history not implemented; needs an external indexer");
        Ok(Vec::new())
    }

    fn estimate_fee(&self, _target_blocks: u32) -> Result<FeeRate> {
        let base = self.rpc.eth_gas_price()?;
        let tip = self.rpc.eth_max_priority_fee_per_gas()?;
        // max_fee = base + tip (simple heuristic; node may include further headroom).
        let max_fee = base.saturating_add(tip);
        Ok(FeeRate::Gwei {
            max_fee,
            max_priority: tip,
            chain: self.params.chain_id,
        })
    }

    fn build_tx(&self, params: SendParams) -> Result<UnsignedTx> {
        let from_str = params.from.as_str();
        let to_bytes = eth_address::from_str_normalized(params.to.as_str())?;

        let nonce = self.rpc.eth_get_transaction_count(from_str)?;
        let base_fee = self.rpc.eth_gas_price()?;
        let tip = self.rpc.eth_max_priority_fee_per_gas()?;
        let max_fee = base_fee.saturating_add(tip);

        let value_wei = params.amount.atoms();
        let value_hex = format!("0x{:x}", value_wei);

        let gas_limit =
            self.rpc
                .eth_estimate_gas(from_str, params.to.as_str(), &value_hex, "0x")?;

        let (max_priority, max_fee_final) = match params.fee {
            FeeRate::Gwei {
                max_fee,
                max_priority,
                ..
            } => (max_priority, max_fee),
            _ => (tip, max_fee),
        };

        let tx = Eip1559Tx {
            chain_id: self.params.eip155_chain_id,
            nonce,
            max_priority_fee_per_gas: max_priority,
            max_fee_per_gas: max_fee_final,
            gas_limit,
            to: to_bytes,
            value_wei,
            data: vec![],
            access_list: vec![],
        };

        let raw =
            serde_json::to_vec(&tx).map_err(|e| Error::Chain(format!("serialize tx: {e}")))?;
        Ok(UnsignedTx {
            chain: self.params.chain_id,
            raw,
        })
    }

    fn sign(&self, tx: UnsignedTx, key: &PrivateKeyBytes) -> Result<SignedTx> {
        let eip_tx: Eip1559Tx = serde_json::from_slice(&tx.raw)
            .map_err(|e| Error::Chain(format!("deserialize tx: {e}")))?;
        let signed_bytes = sign(&eip_tx, &key.0)?;
        Ok(SignedTx {
            chain: self.params.chain_id,
            raw: signed_bytes,
        })
    }

    fn broadcast(&self, tx: SignedTx) -> Result<TxId> {
        let raw_hex = format!("0x{}", hex::encode(&tx.raw));
        let hash = self.rpc.eth_send_raw_transaction(&raw_hex)?;
        Ok(TxId(hash))
    }

    fn derive_private_key(
        &self,
        seed: &[u8; 64],
        account: u32,
        _change: u32,
        index: u32,
    ) -> Result<PrivateKeyBytes> {
        let bytes = derive::derive_secret_key(seed, account, index)?;
        Ok(PrivateKeyBytes(bytes))
    }
}
