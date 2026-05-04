# AGENTS.md

Project conventions for LLM coding agents working on hodl. Consult before making
changes; flag deviations in the PR description.

## Security

### Zeroize every in-memory secret copy, on every exit path

Every byte of an in-memory secret — vault passwords, BIP-39 seeds, raw private
keys, mnemonic strings — must be zeroized on every exit path of every scope that
holds it. Stack memory isn't scrubbed by Rust on drop; a bare `[u8; 32]` left to
fall out of scope persists in the stack slot until something else overwrites it,
exposing the bytes to a future stack-leak or coredump.

**Rules**

- **Worker functions take `&[u8]` (or `&[u8; N]`), not `[u8; N]` by value.** A
  by-value `Copy` array makes a separate stack copy in the callee that the
  caller can't reach to wipe.
- **One owned copy per scope, mutable, zeroized once on the way out.** Pattern
  for thread workers:
  ```rust
  std::thread::spawn(move || {
      let mut seed = seed;            // mutable rebinding of captured array
      let result = worker(&seed);     // pass by ref
      seed.zeroize();                 // one wipe covers the only copy
      let _ = tx.send(result);
  });
  ```
- **Every exit path zeroizes.** `?` propagation traps secrets in error paths —
  either capture the result first then zeroize then match, or zeroize before
  each early return.
- **Don't return a struct that owns a secret** unless the receiving scope is
  explicit about wiping it. `BroadcastPayload { seed, ... }` is OK because
  `broadcast_thread` is the explicit wipe site.
- **Never `let _ = secret;`** to "drop" — `_` doesn't zeroize.
- Test/dev shortcuts (e.g. `KdfParams::testing()`) follow the same rule; don't
  relax it because params are weaker.

### Anti-patterns (caught in past audits)

```rust
// BAD: rebind-then-zeroize. The closure-captured `seed` and the
// worker fn's by-value parameter are SEPARATE copies and stay live.
std::thread::spawn(move || {
    let result = worker(seed);            // by-value: another copy
    let mut seed_copy = seed;             // third copy
    seed_copy.zeroize();                  // only wipes the third
});

// BAD: only the success path zeroizes. Err leaks the seed.
let txid = active.sign_and_broadcast(...)?;
payload.seed.zeroize();
return Ok(txid.0);
```

## UI / TUI

### Blocking ops must show an animated spinner

Any operation that makes the user wait >~200ms (KDF, Electrum query, RPC call,
broadcast, multi-address gap-scan) must render an animated spinner, not a static
"loading…" string. Static placeholders are indistinguishable from a hung
process.

**Rules**

- Off-thread the work (`std::thread::spawn` + `mpsc::channel`) so the event loop
  keeps ticking the spinner.
- Reuse `hodl_tui::spinner::Spinner` — don't roll a new frame array.
- `pending_*: Option<Receiver<…>>` on the screen state; advance the frame on
  `try_recv() == Err(Empty)`; drop the event-poll timeout to ~80ms while
  pending.
- Block user input that depends on the in-flight result (or queue it explicitly)
  so the user can't double-submit.
- When transitioning to a new screen that will immediately do blocking work,
  paint the new screen's spinner state BEFORE starting the work — otherwise the
  previous screen's last frame sticks until the new screen first renders.

### Any operation that makes a user wait, full stop

Even non-network waits (file I/O on slow disks, large form rebuilds) get a
spinner if they cross the 200ms threshold. The rule is about UX predictability,
not about what's blocking.

## Style

- Conventional Commits (`type(scope): subject`).
- `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` +
  `cargo test --workspace` + `cargo deny check` clean before commit.
- `prettier --write` on any markdown edit.
- No emojis in code or commits unless the user explicitly asks.
- Default to writing no comments. Reserve them for non-obvious invariants (the
  WHY), not narration of WHAT the code does.
