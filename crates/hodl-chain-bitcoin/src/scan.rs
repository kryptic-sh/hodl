use hodl_core::error::Result;
use hodl_core::{Address, Amount, Chain};

/// Walk `change` indices from 0 upward, query each address's balance and
/// history via the chain backend, stop after `gap_limit` consecutive empty
/// addresses, and return all addresses that had any activity or balance.
pub fn gap_scan<C: Chain>(
    chain: &C,
    seed: &[u8; 64],
    account: u32,
    // Chain::derive does not expose the change level; gap_scan uses account 0
    // external chain by convention. The parameter is reserved for future use
    // when the Chain trait is extended.
    _change: u32,
    gap_limit: u32,
) -> Result<Vec<(u32, Address, Amount)>> {
    let mut results = Vec::new();
    let mut gap = 0u32;
    let mut index = 0u32;

    loop {
        let addr = chain.derive(seed, account, index)?;
        let balance = chain.balance(&addr)?;
        let history = chain.history(&addr)?;

        let empty = balance.atoms() == 0 && history.is_empty();
        if empty {
            gap += 1;
            if gap >= gap_limit {
                break;
            }
        } else {
            gap = 0;
            results.push((index, addr, balance));
        }
        index += 1;
    }

    Ok(results)
}
