# rok0-guestbook

An on-chain identity registry on Solana in raw sBPF assembly. No Rust, no Anchor — just instructions.

## What it does

Wallets connect and register with a codename and optional message (up to 700 chars). Each registration creates a PDA account that tracks identity, visit count, timestamp, and clearance rank. A three-tier hierarchy (OPERATIVE → OVERSEER → COMMANDER) allows authority holders to promote and remove members.

Six instructions:

- **register** — creates a PDA via System Program CPI, writes identity (authority, codename, message), sets clearance to OPERATIVE
- **heartbeat** — increments visit counter and updates `last_seen` via clock sysvar
- **promote** — COMMANDER-only: elevates a target OPERATIVE to OVERSEER
- **deregister** — closes the account; self-close (2 accounts) or COMMANDER-force (3 accounts), lamports returned to caller
- **update_message** — rewrites the message field in-place using BPF realloc; account resizes to `72 + new_msg_len`
- ~~**bootstrap**~~ — removed after use; see `BOOTSTRAP.md`

## Account layout

PDA seeds: `["overseer", wallet_pubkey]`

```
+0x00  authority   [u8;32]   wallet that registered
+0x20  codename    [u8;16]   e.g. "CASPER", "UNIT-04"
+0x30  enrolled_at  i64      unix timestamp (register)
+0x38  last_seen    i64      unix timestamp (heartbeat)
+0x40  visits       u32      heartbeat counter
+0x44  clearance    u8       0=OPERATIVE 1=OVERSEER 2=COMMANDER
+0x45  bump         u8
+0x46  msg_len      u16      0..=700
+0x48  message     [u8; msg_len]
       total: 72 + msg_len bytes
```

## Assembly

**Input buffer.** Accounts are not fixed-stride. Each slot is `96 + align8(dlen + 10240)` bytes. The entrypoint walks all accounts in a loop, saves each base pointer to a stack slot (`r10-8` = acct0, `r10-16` = acct1, …), and lands on instruction data right after the last account.

**Registers.** `r1`–`r5` are clobbered by any `call`. `r6`–`r9` survive calls and hold values that must outlast a CPI or syscall. `r10` is the frame pointer. After each CPI, account pointers are reloaded from their stack slots.

**CPI structs.** `SolAccountMeta` (16 B), `SolInstruction` (40 B), `SolAccountInfo` (56 B), `SolSignerSeed` (16 B), and `SolSignerSeeds` (16 B) are built by hand on the stack before `sol_invoke_signed_c`. The `SolAccountInfo` array must be contiguous in ascending address order — passing the wrong base pointer causes the runtime to read the wrong struct for `accounts[1]`. `rent_epoch` is read as `*(next_account_base - 8)`.

**PDA validation.** Every instruction that reads from a PDA calls `sol_create_program_address` with `["overseer", authority, [bump]]` and compares the result against the passed account key. Trust nothing — derive and compare.

**Variable-length message.** `register` allocates `72 + msg_len` bytes via `create_account`. `update_message` writes a new `data_len` to `pda_ptr + ACCT_DLEN` in the input buffer — the BPF runtime resizes the on-chain account at the end of the transaction (up to `current_size + 10240` bytes per transaction).

**Helpers.** `cmp32` and `copy32` compare or copy 32-byte pubkeys in four 8-byte loads/stores. `fill_meta` and `fill_acct_info` build CPI structs at an explicit destination pointer since each `call` frame has its own `r10`.

**`.equ` is 32-bit.** Large constants (e.g. 64-bit pubkey words) cannot be stored via `.equ` in llvm-mc's BPF target — they silently truncate. Use `mov64` with a small immediate or write byte-by-byte with `stxb`.

## Build

```bash
sbpf build
```

Assembles `src/rok0_guestbook/rok0_guestbook.s` → `deploy/rok0_guestbook.so`.

## Run

```bash
agave-ledger-tool program run deploy/rok0_guestbook.so \
  --ledger test-ledger \
  --mode interpreter \
  --input src/rok0_guestbook/instructions_register.json \
  --trace trace_register.txt
```

Swap the `--input` file for any of:

```
instructions_register.json
instructions_heartbeat.json
instructions_promote.json
instructions_deregister_self.json
instructions_deregister_commander.json
```

Uses `--mode interpreter`. Each run writes a trace file (agave appends `.0`, e.g. `trace_register.txt.0`).

Each trace line shows all 11 registers before the instruction executes:

```
64 [r0..r9, r10]  31: ldxdw r2, [r10-0x8]
```

Watching `r7` advance after the account-walk loop confirms stride arithmetic. Stack slot values at `r10-8`, `r10-16`, … after the loop confirm saved account pointers. Register dumps before `sol_invoke_signed_c` show exact addresses of `SolInstruction` and `SolAccountInfo` — cross-reference against the stack layout comments in the source.

## Test

```bash
cargo test
```

Uses [litesvm](https://github.com/LiteSVM/litesvm). Tests live in `tests/` — one file per instruction. Each covers success paths and every error code.

| Suite          | Tests |
| -------------- | ----- |
| register       | 6     |
| heartbeat      | 8     |
| promote        | 8     |
| deregister     | 11    |
| update_message | 10    |

## Input files

JSON files under `src/rok0_guestbook/`. Each has `accounts` (ordered list matching program account indices) and `instruction_data` (raw bytes). Discriminator is the first byte of instruction data.

| disc | instruction    | ix_data                                               |
| ---- | -------------- | ----------------------------------------------------- |
| 0    | register       | disc codename[16] bump lamports[8] msg_len[2] message |
| 1    | heartbeat      | disc                                                  |
| 2    | promote        | disc target_wallet[32]                                |
| 3    | deregister     | disc                                                  |
| 5    | update_message | disc msg_len[2] message                               |

## Statistics

Binary: 8368 bytes total, 7104 bytes `.text` (888 instructions × 8 bytes).

Source: 1440 lines, ~926 assembly instructions before assembly.

| Instruction            | CU     | CPIs | notes                             |
| ---------------------- | ------ | ---- | --------------------------------- |
| register               | 3180   | 1    | create_account + clock sysvar     |
| heartbeat              | 1815   | 0    | clock sysvar + PDA validation     |
| promote                | 3282   | 0    | double PDA validation             |
| deregister (self)      | 1737   | 0    | PDA validation + zero loop        |
| deregister (commander) | 3327   | 0    | double PDA validation + zero loop |
| update_message         | 1725   | 0    | PDA validation + copy loop        |
| error paths            | 30–126 | 0    | fail fast                         |

All instructions stay under 0.3% of the 1,400,000 CU budget.

## Error codes

| code | error                |
| ---- | -------------------- |
| 0x01 | invalid instruction  |
| 0x02 | wrong account count  |
| 0x03 | not signer           |
| 0x04 | invalid PDA          |
| 0x05 | CPI failed           |
| 0x06 | wrong owner          |
| 0x07 | account too small    |
| 0x08 | authority mismatch   |
| 0x09 | not commander        |
| 0x0A | not operative        |
| 0x0B | account not writable |
