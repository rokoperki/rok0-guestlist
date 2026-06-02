; ── Input buffer ─────────────────────────────────────
.equ NUM_ACCOUNTS,      0x0000   ; u64 at r1+0
.equ FIRST_ACCT,        0x0008   ; first account base

; ── Per-account fields (offset from account base) ────
.equ ACCT_DUP,          0x00     ; u8 dup_info (0xff = not dup)
.equ ACCT_IS_SIGNER,    0x01     ; u8
.equ ACCT_IS_WRITE,     0x02     ; u8
.equ ACCT_KEY,          0x08     ; [u8;32] pubkey
.equ ACCT_OWNER,        0x28     ; [u8;32] owner program
.equ ACCT_LAMPORTS,     0x48     ; u64
.equ ACCT_DLEN,         0x50     ; u64 data length
.equ ACCT_DATA,         0x58     ; data start
; stride(d) = 96 + align8(d + 10240); rent_epoch is always at next_acct_ptr - 8

; ── Overseer state offsets (within PDA data) ─────────
.equ OS_AUTHORITY,      0x00     ; [u8;32]
.equ OS_CODENAME,       0x20     ; [u8;16]
.equ OS_CODENAME_HI,    0x28     ; [u8;8]  second half of codename
.equ OS_ENROLLED,       0x30     ; i64
.equ OS_LAST_SEEN,      0x38     ; i64
.equ OS_VISITS,         0x40     ; u32
.equ OS_CLEARANCE,      0x44     ; u8
.equ OS_BUMP,           0x45     ; u8
.equ OS_MSG_LEN,        0x46     ; u16 (0..=700)
.equ OS_MESSAGE,        0x48     ; [u8; msg_len]
.equ OS_HEADER,         0x48     ; fixed-size portion (72 bytes)
.equ MSG_MAX,           700

; ── Clearance levels ─────────────────────────────────
.equ CLR_OPERATIVE,     0
.equ CLR_OVERSEER,      1
.equ CLR_COMMANDER,     2

; ── Instruction discriminators ───────────────────────
.equ IX_REGISTER,       0
.equ IX_HEARTBEAT,      1
.equ IX_PROMOTE,        2
.equ IX_DEREGISTER,     3
.equ IX_BOOTSTRAP,      4

; ── Genesis authority (22kQ9csvmpgtaUxR92dsFRtQ6zDEMuT8wwngtBQs21Q2) ─
; stored as 4 × u64 LE for comparison via stxdw + cmp32
.equ GENESIS_W0, 0x6B5F1E62DA564E0F
.equ GENESIS_W1, 0xA60A18462F66436A
.equ GENESIS_W2, 0x92B1828C04E5CFD7
.equ GENESIS_W3, 0xDFC4F8B271019CCE

; ── Seed lengths ─────────────────────────────────────
.equ SEED_OVERSEER_LEN, 8        ; "overseer"
.equ SEED_PUBKEY_LEN,   32
.equ SEED_BUMP_LEN,     1

; ── register stack layout (offsets from r10) ─────────
; Walking loop saves account ptrs:
;   [r10 -  8]  acct0 ptr (authority)
;   [r10 - 16]  acct1 ptr (pda)
;   [r10 - 24]  acct2 ptr (system_program)
;
; Saved ix_data fields:
;   [r10 - 32]  prog_id ptr
;   [r10 - 40]  codename[0..7]
;   [r10 - 48]  codename[8..15]
;   [r10 - 56]  lamports (u64)
;   [r10 - 64]  bump (u8)
;   [r10 - 72]  msg_len (u16 zero-extended to u64)
;
; PDA validation:
;   [r10 - 80]  "overseer" seed string (8 bytes, ptr = r10-80)
;   [r10 -112]  derived_pda output (32 bytes)
;   [r10 -160]  seeds[0..2] for sol_create_program_address (3 × 16 bytes)
;
; System Program create_account CPI:
;   [r10 -216]  ix_data [disc:4, lamports:8, space:8, owner:32] = 52 bytes
;               (also reused as Clock struct after CPI completes)
;   [r10 -248]  meta[0] (authority)
;   [r10 -232]  meta[1] (pda)
;   [r10 -288]  SolInstruction (40 bytes)
;   [r10 -344]  SolAccountInfo[0] (authority, 56 bytes)
;   [r10 -400]  SolAccountInfo[1] (pda, 56 bytes)
;   [r10 -448]  SolSignerSeed[0..2] (3 × 16 bytes)
;   [r10 -464]  SolSignerSeeds (16 bytes)
;   Total: 464 bytes

.globl entrypoint

entrypoint:
    ; ── walk accounts, save ptrs, find ix_data ────────
    ; r6 = num_accounts countdown, r7 = current acct ptr
    ; r9 = save index (zero at entry), r1 = input ptr (preserved)
    ldxdw r6, [r1 + 0]
    mov64 r7, r1
    add64 r7, 8

find_ix_data_loop:
    jeq   r6, 0, find_ix_data_done

    mov64 r3, r9
    lsh64 r3, 3
    mov64 r2, r10
    sub64 r2, 8
    sub64 r2, r3
    stxdw [r2 + 0], r7           ; stack[r9] = account base
    add64 r9, 1

    ldxdw r2, [r7 + ACCT_DLEN]
    add64 r2, 10240
    add64 r2, 7
    mov64 r3, r2
    and64 r3, 7
    sub64 r2, r3
    add64 r2, 96
    add64 r7, r2
    sub64 r6, 1
    ja    find_ix_data_loop

find_ix_data_done:
    ldxdw r3, [r7 + 0]           ; ix_data_len
    jlt   r3, 1, error_invalid_ix

    ldxb  r4, [r7 + 8]           ; discriminator
    jeq   r4, IX_REGISTER,   register_handler
    jeq   r4, IX_HEARTBEAT,  heartbeat_handler
    jeq   r4, IX_PROMOTE,    promote_handler
    jeq   r4, IX_DEREGISTER, deregister_handler
    jeq   r4, IX_BOOTSTRAP,  bootstrap_handler
    ja    error_invalid_ix

; ── register ──────────────────────────────────────────
; accounts:  [authority(0,signer,w)  pda(1,w)  system_program(2)]
; ix_data:   [disc:1  codename:16  bump:1  lamports:8  msg_len:2  message:msg_len]
;            minimum 28 bytes, maximum 728 bytes (700-char message)

register_handler:
    ; account count == 3
    ldxdw r2, [r1 + NUM_ACCOUNTS]
    jne   r2, 3, error_wrong_accounts_number

    ; authority (acct0) is signer and writable
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_SIGNER]
    jne   r2, 1, error_not_signer
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; pda (acct1) is writable
    ldxdw r2, [r10 - 16]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; ix_data_len >= 28 (minimum: no message body)
    ldxdw r3, [r7 + 0]
    jlt   r3, 28, error_invalid_ix

    ; save prog_id ptr (ix_data_start + ix_data_len = r7+8 + r3)
    mov64 r2, r7
    add64 r2, 8
    add64 r2, r3
    stxdw [r10 - 32], r2

    ; save codename[0..7] from ix_data[1..8]
    ldxdw r2, [r7 + 9]
    stxdw [r10 - 40], r2

    ; save codename[8..15] from ix_data[9..16]
    ldxdw r2, [r7 + 17]
    stxdw [r10 - 48], r2

    ; save bump from ix_data[17]
    ldxb  r2, [r7 + 25]
    stxb  [r10 - 64], r2

    ; save lamports from ix_data[18..25]
    ldxdw r2, [r7 + 26]
    stxdw [r10 - 56], r2

    ; read msg_len from ix_data[26..27] (r7+8+26 = r7+34)
    ldxh  r2, [r7 + 34]
    jgt   r2, MSG_MAX, error_invalid_ix
    ; verify ix_data contains the full message: ix_data_len >= 28 + msg_len
    mov64 r4, r2
    add64 r4, 28
    jlt   r3, r4, error_invalid_ix
    stxdw [r10 - 72], r2              ; save msg_len

    ; write "overseer" seed string at r10-80
    ; "overseer" = 6F 76 65 72 73 65 65 72
    mov64 r2, 0x6F
    stxb  [r10 - 80], r2         ; 'o'
    mov64 r2, 0x76
    stxb  [r10 - 79], r2         ; 'v'
    mov64 r2, 0x65
    stxb  [r10 - 78], r2         ; 'e'
    mov64 r2, 0x72
    stxb  [r10 - 77], r2         ; 'r'
    mov64 r2, 0x73
    stxb  [r10 - 76], r2         ; 's'
    mov64 r2, 0x65
    stxb  [r10 - 75], r2         ; 'e'
    mov64 r2, 0x65
    stxb  [r10 - 74], r2         ; 'e'
    mov64 r2, 0x72
    stxb  [r10 - 73], r2         ; 'r'

    ; ── validate PDA via sol_create_program_address ───
    ; seeds[0] = "overseer": ptr=r10-80, len=8
    mov64 r2, r10
    sub64 r2, 80
    stxdw [r10 - 160], r2
    mov64 r2, SEED_OVERSEER_LEN
    stxdw [r10 - 152], r2

    ; seeds[1] = authority.key: ptr=acct0+ACCT_KEY, len=32
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    stxdw [r10 - 144], r2
    mov64 r2, SEED_PUBKEY_LEN
    stxdw [r10 - 136], r2

    ; seeds[2] = bump: ptr=r10-64, len=1
    mov64 r2, r10
    sub64 r2, 64
    stxdw [r10 - 128], r2
    mov64 r2, SEED_BUMP_LEN
    stxdw [r10 - 120], r2

    ; sol_create_program_address(seeds, 3, prog_id, out_pda)
    mov64 r1, r10
    sub64 r1, 160
    mov64 r2, 3
    ldxdw r3, [r10 - 32]
    mov64 r4, r10
    sub64 r4, 112
    call  sol_create_program_address
    jne   r0, 0, error_invalid_pda

    ; compare derived_pda with pda.key (acct1)
    mov64 r1, r10
    sub64 r1, 112
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_invalid_pda

    ; ── CPI: System Program create_account ────────────
    ; ix_data at r10-216: disc(u32 LE=0) + lamports(u64) + space(u64) + owner([u8;32])

    ; disc = 0
    mov64 r2, 0
    stxw  [r10 - 216], r2

    ; lamports
    ldxdw r2, [r10 - 56]
    stxdw [r10 - 212], r2

    ; space = OS_HEADER + msg_len (dynamic)
    ldxdw r2, [r10 - 72]
    add64 r2, OS_HEADER
    stxdw [r10 - 204], r2

    ; owner = program_id (32 bytes at r10-196)
    mov64 r1, r10
    sub64 r1, 196
    ldxdw r2, [r10 - 32]
    call  copy32

    ; meta[0] = authority (payer): writable=1, signer=1
    mov64 r1, r10
    sub64 r1, 248
    ldxdw r2, [r10 - 8]
    mov64 r3, 1
    mov64 r4, 1
    call  fill_meta

    ; meta[1] = pda (new account): writable=1, signer=1
    mov64 r1, r10
    sub64 r1, 232
    ldxdw r2, [r10 - 16]
    mov64 r3, 1
    mov64 r4, 1
    call  fill_meta

    ; SolInstruction
    ldxdw r2, [r10 - 24]          ; acct2 (system_program)
    add64 r2, ACCT_KEY
    stxdw [r10 - 288], r2          ; program_id_ptr = system_program.key
    mov64 r2, r10
    sub64 r2, 248
    stxdw [r10 - 280], r2          ; accounts_ptr = &meta[0]
    mov64 r2, 2
    stxdw [r10 - 272], r2          ; accounts_len = 2
    mov64 r2, r10
    sub64 r2, 216
    stxdw [r10 - 264], r2          ; data_ptr = &ix_data
    mov64 r2, 52
    stxdw [r10 - 256], r2          ; data_len = 52

    ; SolAccountInfo[0] = authority (acct0) — at r10-400 (lower addr = array base)
    mov64 r1, r10
    sub64 r1, 400
    ldxdw r2, [r10 - 8]
    ldxdw r3, [r10 - 16]
    mov64 r4, 1
    mov64 r5, 1
    call  fill_acct_info

    ; SolAccountInfo[1] = pda (acct1) — at r10-344 (= r10-400 + 56)
    mov64 r1, r10
    sub64 r1, 344
    ldxdw r2, [r10 - 16]
    ldxdw r3, [r10 - 24]
    mov64 r4, 1
    mov64 r5, 1
    call  fill_acct_info

    ; signer seeds for PDA (same 3 seeds as validation)
    ; SolSignerSeed[0] = "overseer"
    mov64 r2, r10
    sub64 r2, 80
    stxdw [r10 - 448], r2
    mov64 r2, SEED_OVERSEER_LEN
    stxdw [r10 - 440], r2

    ; SolSignerSeed[1] = authority.key
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    stxdw [r10 - 432], r2
    mov64 r2, SEED_PUBKEY_LEN
    stxdw [r10 - 424], r2

    ; SolSignerSeed[2] = bump
    mov64 r2, r10
    sub64 r2, 64
    stxdw [r10 - 416], r2
    mov64 r2, SEED_BUMP_LEN
    stxdw [r10 - 408], r2

    ; SolSignerSeeds: 1 signer, 3 seeds
    mov64 r2, r10
    sub64 r2, 448
    stxdw [r10 - 464], r2          ; seeds_arr_ptr
    mov64 r2, 3
    stxdw [r10 - 456], r2          ; seeds_arr_len

    ; CPI call
    mov64 r1, r10
    sub64 r1, 288                  ; &SolInstruction
    mov64 r2, r10
    sub64 r2, 400                  ; &SolAccountInfo[0] (lower addr = array base)
    mov64 r3, 2
    mov64 r4, r10
    sub64 r4, 464                  ; &SolSignerSeeds[0]
    mov64 r5, 1
    call  sol_invoke_signed_c
    jne   r0, 0, error_cpi_failed

    ; ── write overseer state ──────────────────────────
    ; r6 = pda.data base, preserved across copy32/clock calls
    ldxdw r6, [r10 - 16]
    add64 r6, ACCT_DATA

    ; OS_AUTHORITY = authority.key
    mov64 r1, r6
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    call  copy32

    ; OS_CODENAME = codename[0..7]
    ldxdw r2, [r10 - 40]
    stxdw [r6 + OS_CODENAME], r2

    ; OS_CODENAME second half = codename[8..15]
    ldxdw r2, [r10 - 48]
    stxdw [r6 + OS_CODENAME_HI], r2

    ; OS_ENROLLED = OS_LAST_SEEN = Clock.unix_timestamp
    ; reuse ix_data area (r10-216) as Clock struct (40 bytes, offset 0x20 = unix_timestamp)
    mov64 r1, r10
    sub64 r1, 216
    call  sol_get_clock_sysvar
    ldxdw r2, [r10 - 184]          ; Clock+0x20 = r10-216+32 = r10-184
    stxdw [r6 + OS_ENROLLED], r2
    stxdw [r6 + OS_LAST_SEEN], r2

    ; OS_VISITS = 0
    mov64 r2, 0
    stxw  [r6 + OS_VISITS], r2

    ; OS_CLEARANCE = 0 (OPERATIVE)
    stxb  [r6 + OS_CLEARANCE], r2

    ; OS_BUMP = bump
    ldxb  r2, [r10 - 64]
    stxb  [r6 + OS_BUMP], r2

    ; OS_MSG_LEN = msg_len (u16 LE)
    ldxdw r2, [r10 - 72]
    stxh  [r6 + OS_MSG_LEN], r2

    ; copy message: src = ix_data[28] = r7+36, dst = pda.data + OS_MESSAGE
    ; r8 = src ptr, r9 = dst ptr, r3 = remaining bytes
    mov64 r8, r7
    add64 r8, 36
    mov64 r9, r6
    add64 r9, OS_MESSAGE
    ldxdw r3, [r10 - 72]

reg_msg_dw:
    jlt   r3, 8, reg_msg_b
    ldxdw r2, [r8 + 0]
    stxdw [r9 + 0], r2
    add64 r8, 8
    add64 r9, 8
    sub64 r3, 8
    ja    reg_msg_dw

reg_msg_b:
    jeq   r3, 0, reg_msg_done
    ldxb  r2, [r8 + 0]
    stxb  [r9 + 0], r2
    add64 r8, 1
    add64 r9, 1
    sub64 r3, 1
    ja    reg_msg_b

reg_msg_done:
    mov64 r0, 0
    exit

; ── heartbeat ─────────────────────────────────────────
; accounts:  [authority(0,signer)  pda(1,w)]
; ix_data:   [disc:1]
;
; stack layout:
;   [r10 -  8]  acct0 ptr (authority)
;   [r10 - 16]  acct1 ptr (pda)
;   [r10 - 24]  prog_id ptr
;   [r10 - 32]  "overseer" seed string (8 bytes, ptr = r10-32)
;   [r10 - 40]  bump (u8)
;   [r10 - 72]  derived_pda output (32 bytes)
;   [r10 -120]  seeds[0..2] (3 × 16 bytes)
;   [r10 -160]  clock struct (40 bytes, unix_timestamp at +0x20 = r10-128)

heartbeat_handler:
    ; account count == 2
    ldxdw r2, [r1 + NUM_ACCOUNTS]
    jne   r2, 2, error_wrong_accounts_number

    ; authority (acct0) is signer
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_SIGNER]
    jne   r2, 1, error_not_signer

    ; pda (acct1) is writable
    ldxdw r2, [r10 - 16]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; save prog_id ptr (r7+8+ix_data_len)
    ldxdw r3, [r7 + 0]
    mov64 r2, r7
    add64 r2, 8
    add64 r2, r3
    stxdw [r10 - 24], r2

    ; pda.owner == program_id
    ldxdw r1, [r10 - 24]           ; prog_id ptr
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_OWNER
    call  cmp32
    jne   r0, 0, error_wrong_owner

    ; pda.data_len >= OS_HEADER
    ldxdw r2, [r10 - 16]
    ldxdw r2, [r2 + ACCT_DLEN]
    jlt   r2, OS_HEADER, error_wrong_size

    ; pda.data.authority == authority.key
    ldxdw r1, [r10 - 16]
    add64 r1, ACCT_DATA             ; r1 = &pda.data[OS_AUTHORITY]
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_authority_mismatch

    ; read bump from pda.data[OS_BUMP]
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_BUMP]
    stxb  [r10 - 40], r2

    ; write "overseer" seed string at r10-32
    mov64 r2, 0x6F
    stxb  [r10 - 32], r2
    mov64 r2, 0x76
    stxb  [r10 - 31], r2
    mov64 r2, 0x65
    stxb  [r10 - 30], r2
    mov64 r2, 0x72
    stxb  [r10 - 29], r2
    mov64 r2, 0x73
    stxb  [r10 - 28], r2
    mov64 r2, 0x65
    stxb  [r10 - 27], r2
    mov64 r2, 0x65
    stxb  [r10 - 26], r2
    mov64 r2, 0x72
    stxb  [r10 - 25], r2

    ; seeds[0] = "overseer"
    mov64 r2, r10
    sub64 r2, 32
    stxdw [r10 - 120], r2
    mov64 r2, SEED_OVERSEER_LEN
    stxdw [r10 - 112], r2

    ; seeds[1] = authority.key
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    stxdw [r10 - 104], r2
    mov64 r2, SEED_PUBKEY_LEN
    stxdw [r10 - 96], r2

    ; seeds[2] = bump
    mov64 r2, r10
    sub64 r2, 40
    stxdw [r10 - 88], r2
    mov64 r2, SEED_BUMP_LEN
    stxdw [r10 - 80], r2

    ; sol_create_program_address(seeds, 3, prog_id, out_pda)
    mov64 r1, r10
    sub64 r1, 120
    mov64 r2, 3
    ldxdw r3, [r10 - 24]
    mov64 r4, r10
    sub64 r4, 72
    call  sol_create_program_address
    jne   r0, 0, error_invalid_pda

    ; compare derived_pda with pda.key
    mov64 r1, r10
    sub64 r1, 72
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_invalid_pda

    ; visits++
    ldxdw r6, [r10 - 16]
    add64 r6, ACCT_DATA             ; r6 = pda.data base (preserved across syscall)
    ldxw  r2, [r6 + OS_VISITS]
    add64 r2, 1
    stxw  [r6 + OS_VISITS], r2

    ; last_seen = Clock.unix_timestamp
    mov64 r1, r10
    sub64 r1, 160
    call  sol_get_clock_sysvar
    ldxdw r2, [r10 - 128]          ; Clock+0x20 = r10-160+32
    stxdw [r6 + OS_LAST_SEEN], r2

    mov64 r0, 0
    exit

; ── promote ───────────────────────────────────────────
; accounts:  [commander(0,signer)  commander_pda(1)  target_pda(2,w)]
; ix_data:   [disc:2  target_wallet:32] = 33 bytes
;
; stack layout:
;   [r10 -  8]  acct0 ptr (commander)
;   [r10 - 16]  acct1 ptr (commander_pda)
;   [r10 - 24]  acct2 ptr (target_pda)
;   [r10 - 32]  prog_id ptr
;   [r10 - 40]  target_wallet[0..7]
;   [r10 - 48]  target_wallet[8..15]
;   [r10 - 56]  target_wallet[16..23]
;   [r10 - 64]  target_wallet[24..31]
;   [r10 - 72]  "overseer" seed string (8 bytes, ptr = r10-72)
;   [r10 - 80]  bump (u8, reused for both PDAs)
;   [r10 -112]  derived_pda output (32 bytes, reused)
;   [r10 -160]  seeds[0..2] (48 bytes, seeds[1].ptr updated per PDA)

promote_handler:
    ; account count == 3
    ldxdw r2, [r1 + NUM_ACCOUNTS]
    jne   r2, 3, error_wrong_accounts_number

    ; commander (acct0) is signer
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_SIGNER]
    jne   r2, 1, error_not_signer

    ; target_pda (acct2) is writable
    ldxdw r2, [r10 - 24]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; ix_data_len >= 33
    ldxdw r3, [r7 + 0]
    jlt   r3, 33, error_invalid_ix

    ; save prog_id ptr
    mov64 r2, r7
    add64 r2, 8
    add64 r2, r3
    stxdw [r10 - 32], r2

    ; save target_wallet from ix_data[1..32] (r7+9)
    ; stored in ASCENDING address order starting at r10-64 so cmp32
    ; can walk [r2+0..r2+24] contiguously:
    ;   r10-64 = target_wallet[0..7]   (lowest addr)
    ;   r10-56 = target_wallet[8..15]
    ;   r10-48 = target_wallet[16..23]
    ;   r10-40 = target_wallet[24..31] (highest addr)
    ldxdw r2, [r7 + 9]
    stxdw [r10 - 64], r2
    ldxdw r2, [r7 + 17]
    stxdw [r10 - 56], r2
    ldxdw r2, [r7 + 25]
    stxdw [r10 - 48], r2
    ldxdw r2, [r7 + 33]
    stxdw [r10 - 40], r2

    ; write "overseer" seed string at r10-72
    mov64 r2, 0x6F
    stxb  [r10 - 72], r2
    mov64 r2, 0x76
    stxb  [r10 - 71], r2
    mov64 r2, 0x65
    stxb  [r10 - 70], r2
    mov64 r2, 0x72
    stxb  [r10 - 69], r2
    mov64 r2, 0x73
    stxb  [r10 - 68], r2
    mov64 r2, 0x65
    stxb  [r10 - 67], r2
    mov64 r2, 0x65
    stxb  [r10 - 66], r2
    mov64 r2, 0x72
    stxb  [r10 - 65], r2

    ; build seeds skeleton (seeds[0] and seeds[2] don't change between validations)
    ; seeds[0] = "overseer"
    mov64 r2, r10
    sub64 r2, 72
    stxdw [r10 - 160], r2
    mov64 r2, SEED_OVERSEER_LEN
    stxdw [r10 - 152], r2
    ; seeds[1].len = 32 (ptr updated per PDA)
    mov64 r2, SEED_PUBKEY_LEN
    stxdw [r10 - 136], r2
    ; seeds[2] = bump at r10-80
    mov64 r2, r10
    sub64 r2, 80
    stxdw [r10 - 128], r2
    mov64 r2, SEED_BUMP_LEN
    stxdw [r10 - 120], r2

    ; ── validate commander_pda ────────────────────────────

    ; commander_pda.owner == program_id
    ldxdw r1, [r10 - 32]
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_OWNER
    call  cmp32
    jne   r0, 0, error_wrong_owner

    ; commander_pda.data_len >= OS_HEADER
    ldxdw r2, [r10 - 16]
    ldxdw r2, [r2 + ACCT_DLEN]
    jlt   r2, OS_HEADER, error_wrong_size

    ; commander_pda.authority == commander.key
    ldxdw r1, [r10 - 16]
    add64 r1, ACCT_DATA
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_authority_mismatch

    ; seeds[1].ptr = commander.key
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    stxdw [r10 - 144], r2

    ; read commander_pda bump → r10-80
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_BUMP]
    stxb  [r10 - 80], r2

    ; sol_create_program_address for commander_pda
    mov64 r1, r10
    sub64 r1, 160
    mov64 r2, 3
    ldxdw r3, [r10 - 32]
    mov64 r4, r10
    sub64 r4, 112
    call  sol_create_program_address
    jne   r0, 0, error_invalid_pda

    ; compare derived with commander_pda.key
    mov64 r1, r10
    sub64 r1, 112
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_invalid_pda

    ; commander_pda.clearance == COMMANDER (2)
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_CLEARANCE]
    jne   r2, CLR_COMMANDER, error_not_commander

    ; ── validate target_pda ───────────────────────────────

    ; target_pda.owner == program_id
    ldxdw r1, [r10 - 32]
    ldxdw r2, [r10 - 24]
    add64 r2, ACCT_OWNER
    call  cmp32
    jne   r0, 0, error_wrong_owner

    ; target_pda.data_len >= OS_HEADER
    ldxdw r2, [r10 - 24]
    ldxdw r2, [r2 + ACCT_DLEN]
    jlt   r2, OS_HEADER, error_wrong_size

    ; target_pda.authority == target_wallet (ascending from r10-64)
    ldxdw r1, [r10 - 24]
    add64 r1, ACCT_DATA
    mov64 r2, r10
    sub64 r2, 64                    ; r10-64 = target_wallet[0..7] start
    call  cmp32
    jne   r0, 0, error_authority_mismatch

    ; seeds[1].ptr = target_wallet (r10-64)
    mov64 r2, r10
    sub64 r2, 64
    stxdw [r10 - 144], r2

    ; read target_pda bump → r10-80 (reuse)
    ldxdw r2, [r10 - 24]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_BUMP]
    stxb  [r10 - 80], r2

    ; sol_create_program_address for target_pda
    mov64 r1, r10
    sub64 r1, 160
    mov64 r2, 3
    ldxdw r3, [r10 - 32]
    mov64 r4, r10
    sub64 r4, 112
    call  sol_create_program_address
    jne   r0, 0, error_invalid_pda

    ; compare derived with target_pda.key
    mov64 r1, r10
    sub64 r1, 112
    ldxdw r2, [r10 - 24]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_invalid_pda

    ; target_pda.clearance == OPERATIVE (0)
    ldxdw r2, [r10 - 24]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_CLEARANCE]
    jne   r2, CLR_OPERATIVE, error_not_operative

    ; ── promote: OPERATIVE → OVERSEER ────────────────────
    ldxdw r2, [r10 - 24]
    add64 r2, ACCT_DATA
    mov64 r3, CLR_OVERSEER
    stxb  [r2 + OS_CLEARANCE], r3

    mov64 r0, 0
    exit

; ── deregister (stub) ─────────────────────────────────
; ── deregister ────────────────────────────────────────
; Case A (2 accounts): [authority(signer,w)  pda(w)]
;   lamports → authority, data zeroed
; Case B (3 accounts): [commander(signer)  commander_pda(r)  target_pda(w)]
;   lamports → commander, data zeroed
; ix_data: [disc:3]

deregister_handler:
    ldxdw r2, [r1 + NUM_ACCOUNTS]
    jeq   r2, 2, deregister_self
    jeq   r2, 3, deregister_commander
    ja    error_wrong_accounts_number

; ── Case A: self-deregister ───────────────────────────
; stack layout (same as heartbeat):
;   [r10 -  8]  acct0 ptr (authority)
;   [r10 - 16]  acct1 ptr (pda)
;   [r10 - 24]  prog_id ptr
;   [r10 - 32]  "overseer" (8 bytes, ptr = r10-32)
;   [r10 - 40]  bump (u8)
;   [r10 - 72]  derived_pda (32 bytes)
;   [r10 -120]  seeds[0..2] (48 bytes)

deregister_self:
    ; authority (acct0) is signer and writable
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_SIGNER]
    jne   r2, 1, error_not_signer
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; pda (acct1) is writable
    ldxdw r2, [r10 - 16]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; save prog_id ptr
    ldxdw r3, [r7 + 0]
    mov64 r2, r7
    add64 r2, 8
    add64 r2, r3
    stxdw [r10 - 24], r2

    ; pda.owner == program_id
    ldxdw r1, [r10 - 24]
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_OWNER
    call  cmp32
    jne   r0, 0, error_wrong_owner

    ; pda.data_len >= OS_HEADER
    ldxdw r2, [r10 - 16]
    ldxdw r2, [r2 + ACCT_DLEN]
    jlt   r2, OS_HEADER, error_wrong_size

    ; pda.authority == authority.key
    ldxdw r1, [r10 - 16]
    add64 r1, ACCT_DATA
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_authority_mismatch

    ; write "overseer" at r10-32
    mov64 r2, 0x6F
    stxb  [r10 - 32], r2
    mov64 r2, 0x76
    stxb  [r10 - 31], r2
    mov64 r2, 0x65
    stxb  [r10 - 30], r2
    mov64 r2, 0x72
    stxb  [r10 - 29], r2
    mov64 r2, 0x73
    stxb  [r10 - 28], r2
    mov64 r2, 0x65
    stxb  [r10 - 27], r2
    mov64 r2, 0x65
    stxb  [r10 - 26], r2
    mov64 r2, 0x72
    stxb  [r10 - 25], r2

    ; read bump
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_BUMP]
    stxb  [r10 - 40], r2

    ; seeds[0] = "overseer"
    mov64 r2, r10
    sub64 r2, 32
    stxdw [r10 - 120], r2
    mov64 r2, SEED_OVERSEER_LEN
    stxdw [r10 - 112], r2

    ; seeds[1] = authority.key
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    stxdw [r10 - 104], r2
    mov64 r2, SEED_PUBKEY_LEN
    stxdw [r10 - 96], r2

    ; seeds[2] = bump at r10-40
    mov64 r2, r10
    sub64 r2, 40
    stxdw [r10 - 88], r2
    mov64 r2, SEED_BUMP_LEN
    stxdw [r10 - 80], r2

    ; sol_create_program_address
    mov64 r1, r10
    sub64 r1, 120
    mov64 r2, 3
    ldxdw r3, [r10 - 24]
    mov64 r4, r10
    sub64 r4, 72
    call  sol_create_program_address
    jne   r0, 0, error_invalid_pda

    ; compare derived with pda.key
    mov64 r1, r10
    sub64 r1, 72
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_invalid_pda

    ; transfer lamports: authority.lamports += pda.lamports; pda.lamports = 0
    ldxdw r6, [r10 - 16]                ; r6 = pda ptr
    ldxdw r3, [r6 + ACCT_LAMPORTS]     ; pda.lamports
    ldxdw r2, [r10 - 8]
    ldxdw r4, [r2 + ACCT_LAMPORTS]
    add64 r4, r3
    stxdw [r2 + ACCT_LAMPORTS], r4     ; authority.lamports += pda.lamports
    mov64 r3, 0
    stxdw [r6 + ACCT_LAMPORTS], r3     ; pda.lamports = 0

    ; zero pda data
    ldxdw r8, [r6 + ACCT_DLEN]
    mov64 r9, r6
    add64 r9, ACCT_DATA

self_zero_dw:
    jlt   r8, 8, self_zero_b
    mov64 r2, 0
    stxdw [r9 + 0], r2
    add64 r9, 8
    sub64 r8, 8
    ja    self_zero_dw

self_zero_b:
    jeq   r8, 0, self_zero_done
    mov64 r2, 0
    stxb  [r9 + 0], r2
    add64 r9, 1
    sub64 r8, 1
    ja    self_zero_b

self_zero_done:
    mov64 r0, 0
    exit

; ── Case B: commander-deregister ─────────────────────
; stack layout:
;   [r10 -  8]  acct0 ptr (commander)
;   [r10 - 16]  acct1 ptr (commander_pda)
;   [r10 - 24]  acct2 ptr (target_pda)
;   [r10 - 32]  prog_id ptr
;   [r10 - 40]  target_wallet[24..31]  ← ascending from r10-64
;   [r10 - 48]  target_wallet[16..23]
;   [r10 - 56]  target_wallet[8..15]
;   [r10 - 64]  target_wallet[0..7]
;   [r10 - 72]  "overseer" (8 bytes, ptr = r10-72)
;   [r10 - 80]  bump (u8, reused for both PDAs)
;   [r10 -112]  derived_pda (32 bytes, reused)
;   [r10 -160]  seeds[0..2] (48 bytes)

deregister_commander:
    ; commander (acct0) is signer and writable (receives lamports)
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_SIGNER]
    jne   r2, 1, error_not_signer
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; target_pda (acct2) is writable
    ldxdw r2, [r10 - 24]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; save prog_id ptr
    ldxdw r3, [r7 + 0]
    mov64 r2, r7
    add64 r2, 8
    add64 r2, r3
    stxdw [r10 - 32], r2

    ; write "overseer" at r10-72
    mov64 r2, 0x6F
    stxb  [r10 - 72], r2
    mov64 r2, 0x76
    stxb  [r10 - 71], r2
    mov64 r2, 0x65
    stxb  [r10 - 70], r2
    mov64 r2, 0x72
    stxb  [r10 - 69], r2
    mov64 r2, 0x73
    stxb  [r10 - 68], r2
    mov64 r2, 0x65
    stxb  [r10 - 67], r2
    mov64 r2, 0x65
    stxb  [r10 - 66], r2
    mov64 r2, 0x72
    stxb  [r10 - 65], r2

    ; build seeds skeleton (seeds[1].ptr updated per PDA)
    mov64 r2, r10
    sub64 r2, 72
    stxdw [r10 - 160], r2
    mov64 r2, SEED_OVERSEER_LEN
    stxdw [r10 - 152], r2
    mov64 r2, SEED_PUBKEY_LEN
    stxdw [r10 - 136], r2
    mov64 r2, r10
    sub64 r2, 80
    stxdw [r10 - 128], r2
    mov64 r2, SEED_BUMP_LEN
    stxdw [r10 - 120], r2

    ; ── validate commander_pda ────────────────────────

    ; commander_pda.owner == program_id
    ldxdw r1, [r10 - 32]
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_OWNER
    call  cmp32
    jne   r0, 0, error_wrong_owner

    ; commander_pda.data_len >= OS_HEADER
    ldxdw r2, [r10 - 16]
    ldxdw r2, [r2 + ACCT_DLEN]
    jlt   r2, OS_HEADER, error_wrong_size

    ; commander_pda.authority == commander.key
    ldxdw r1, [r10 - 16]
    add64 r1, ACCT_DATA
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_authority_mismatch

    ; seeds[1] = commander.key
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    stxdw [r10 - 144], r2

    ; read commander_pda bump
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_BUMP]
    stxb  [r10 - 80], r2

    ; sol_create_program_address for commander_pda
    mov64 r1, r10
    sub64 r1, 160
    mov64 r2, 3
    ldxdw r3, [r10 - 32]
    mov64 r4, r10
    sub64 r4, 112
    call  sol_create_program_address
    jne   r0, 0, error_invalid_pda

    ; compare with commander_pda.key
    mov64 r1, r10
    sub64 r1, 112
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_invalid_pda

    ; commander_pda.clearance == COMMANDER (2)
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_CLEARANCE]
    jne   r2, CLR_COMMANDER, error_not_commander

    ; ── validate target_pda ───────────────────────────

    ; target_pda.owner == program_id
    ldxdw r1, [r10 - 32]
    ldxdw r2, [r10 - 24]
    add64 r2, ACCT_OWNER
    call  cmp32
    jne   r0, 0, error_wrong_owner

    ; target_pda.data_len >= OS_HEADER
    ldxdw r2, [r10 - 24]
    ldxdw r2, [r2 + ACCT_DLEN]
    jlt   r2, OS_HEADER, error_wrong_size

    ; read target_wallet from target_pda.data[OS_AUTHORITY] → ascending from r10-64
    ldxdw r2, [r10 - 24]
    add64 r2, ACCT_DATA
    ldxdw r3, [r2 + 0]
    stxdw [r10 - 64], r3
    ldxdw r3, [r2 + 8]
    stxdw [r10 - 56], r3
    ldxdw r3, [r2 + 16]
    stxdw [r10 - 48], r3
    ldxdw r3, [r2 + 24]
    stxdw [r10 - 40], r3

    ; seeds[1] = target_wallet (r10-64)
    mov64 r2, r10
    sub64 r2, 64
    stxdw [r10 - 144], r2

    ; read target_pda bump
    ldxdw r2, [r10 - 24]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_BUMP]
    stxb  [r10 - 80], r2

    ; sol_create_program_address for target_pda
    mov64 r1, r10
    sub64 r1, 160
    mov64 r2, 3
    ldxdw r3, [r10 - 32]
    mov64 r4, r10
    sub64 r4, 112
    call  sol_create_program_address
    jne   r0, 0, error_invalid_pda

    ; compare with target_pda.key
    mov64 r1, r10
    sub64 r1, 112
    ldxdw r2, [r10 - 24]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_invalid_pda

    ; transfer lamports: commander.lamports += target_pda.lamports; target_pda.lamports = 0
    ldxdw r6, [r10 - 24]                ; r6 = target_pda ptr
    ldxdw r3, [r6 + ACCT_LAMPORTS]     ; target_pda.lamports
    ldxdw r2, [r10 - 8]
    ldxdw r4, [r2 + ACCT_LAMPORTS]
    add64 r4, r3
    stxdw [r2 + ACCT_LAMPORTS], r4     ; commander.lamports += target_pda.lamports
    mov64 r3, 0
    stxdw [r6 + ACCT_LAMPORTS], r3     ; target_pda.lamports = 0

    ; zero target_pda data
    ldxdw r8, [r6 + ACCT_DLEN]
    mov64 r9, r6
    add64 r9, ACCT_DATA

cmd_zero_dw:
    jlt   r8, 8, cmd_zero_b
    mov64 r2, 0
    stxdw [r9 + 0], r2
    add64 r9, 8
    sub64 r8, 8
    ja    cmd_zero_dw

cmd_zero_b:
    jeq   r8, 0, cmd_zero_done
    mov64 r2, 0
    stxb  [r9 + 0], r2
    add64 r9, 1
    sub64 r8, 1
    ja    cmd_zero_b

cmd_zero_done:
    mov64 r0, 0
    exit

; ── bootstrap ─────────────────────────────────────────
; One-time instruction: elevates genesis wallet to COMMANDER.
; accounts:  [genesis(0,signer,w)  genesis_pda(1,w)]
; ix_data:   [disc:1]
;
; stack layout:
;   [r10 -  8]  acct0 ptr (genesis wallet)
;   [r10 - 16]  acct1 ptr (genesis pda)
;   [r10 - 24]  prog_id ptr
;   [r10 - 64]  genesis pubkey[0..7]   ← ascending, cmp32 ptr = r10-64
;   [r10 - 56]  genesis pubkey[8..15]
;   [r10 - 48]  genesis pubkey[16..23]
;   [r10 - 40]  genesis pubkey[24..31]
;   [r10 - 72]  "overseer" string (8 bytes, r10-72..r10-65)
;   [r10 - 80]  bump (u8)
;   [r10 -112]  derived_pda (32 bytes, r10-112..r10-81)
;   [r10 -160]  seeds[0..2] (48 bytes)

bootstrap_handler:
    ; account count == 2
    ldxdw r2, [r1 + NUM_ACCOUNTS]
    jne   r2, 2, error_wrong_accounts_number

    ; genesis (acct0) is signer and writable
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_SIGNER]
    jne   r2, 1, error_not_signer
    ldxdw r2, [r10 - 8]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; genesis_pda (acct1) is writable
    ldxdw r2, [r10 - 16]
    ldxb  r2, [r2 + ACCT_IS_WRITE]
    jne   r2, 1, error_not_writable

    ; save prog_id ptr
    ldxdw r3, [r7 + 0]
    mov64 r2, r7
    add64 r2, 8
    add64 r2, r3
    stxdw [r10 - 24], r2

    ; write genesis pubkey (4 × u64 LE) ascending from r10-64
    mov64 r2, GENESIS_W0
    stxdw [r10 - 64], r2
    mov64 r2, GENESIS_W1
    stxdw [r10 - 56], r2
    mov64 r2, GENESIS_W2
    stxdw [r10 - 48], r2
    mov64 r2, GENESIS_W3
    stxdw [r10 - 40], r2

    ; signer.key == hardcoded genesis pubkey
    ldxdw r1, [r10 - 8]
    add64 r1, ACCT_KEY
    mov64 r2, r10
    sub64 r2, 64
    call  cmp32
    jne   r0, 0, error_not_genesis

    ; genesis_pda.owner == program_id
    ldxdw r1, [r10 - 24]
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_OWNER
    call  cmp32
    jne   r0, 0, error_wrong_owner

    ; genesis_pda.data_len >= OS_HEADER (must be registered first)
    ldxdw r2, [r10 - 16]
    ldxdw r2, [r2 + ACCT_DLEN]
    jlt   r2, OS_HEADER, error_wrong_size

    ; not already a COMMANDER
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_CLEARANCE]
    jeq   r2, CLR_COMMANDER, error_already_bootstrapped

    ; write "overseer" at r10-72..r10-65
    mov64 r2, 0x6F
    stxb  [r10 - 72], r2
    mov64 r2, 0x76
    stxb  [r10 - 71], r2
    mov64 r2, 0x65
    stxb  [r10 - 70], r2
    mov64 r2, 0x72
    stxb  [r10 - 69], r2
    mov64 r2, 0x73
    stxb  [r10 - 68], r2
    mov64 r2, 0x65
    stxb  [r10 - 67], r2
    mov64 r2, 0x65
    stxb  [r10 - 66], r2
    mov64 r2, 0x72
    stxb  [r10 - 65], r2

    ; read bump from genesis_pda.data[OS_BUMP]
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_DATA
    ldxb  r2, [r2 + OS_BUMP]
    stxb  [r10 - 80], r2

    ; seeds[0] = "overseer"
    mov64 r2, r10
    sub64 r2, 72
    stxdw [r10 - 160], r2
    mov64 r2, SEED_OVERSEER_LEN
    stxdw [r10 - 152], r2

    ; seeds[1] = genesis.key
    ldxdw r2, [r10 - 8]
    add64 r2, ACCT_KEY
    stxdw [r10 - 144], r2
    mov64 r2, SEED_PUBKEY_LEN
    stxdw [r10 - 136], r2

    ; seeds[2] = bump at r10-80
    mov64 r2, r10
    sub64 r2, 80
    stxdw [r10 - 128], r2
    mov64 r2, SEED_BUMP_LEN
    stxdw [r10 - 120], r2

    ; sol_create_program_address(seeds, 3, prog_id, out_pda@r10-112)
    mov64 r1, r10
    sub64 r1, 160
    mov64 r2, 3
    ldxdw r3, [r10 - 24]
    mov64 r4, r10
    sub64 r4, 112
    call  sol_create_program_address
    jne   r0, 0, error_invalid_pda

    ; compare derived with genesis_pda.key
    mov64 r1, r10
    sub64 r1, 112
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_KEY
    call  cmp32
    jne   r0, 0, error_invalid_pda

    ; set clearance = COMMANDER
    ldxdw r2, [r10 - 16]
    add64 r2, ACCT_DATA
    mov64 r3, CLR_COMMANDER
    stxb  [r2 + OS_CLEARANCE], r3

    mov64 r0, 0
    exit

; ── Error codes ───────────────────────────────────────
error_invalid_ix:
    mov64 r0, 0x01
    exit

error_wrong_accounts_number:
    mov64 r0, 0x02
    exit

error_not_signer:
    mov64 r0, 0x03
    exit

error_invalid_pda:
    mov64 r0, 0x04
    exit

error_cpi_failed:
    mov64 r0, 0x05
    exit

error_wrong_owner:
    mov64 r0, 0x06
    exit

error_wrong_size:
    mov64 r0, 0x07
    exit

error_authority_mismatch:
    mov64 r0, 0x08
    exit

error_not_commander:
    mov64 r0, 0x09
    exit

error_not_operative:
    mov64 r0, 0x0A
    exit

error_not_writable:
    mov64 r0, 0x0B
    exit

error_not_genesis:
    mov64 r0, 0x0C
    exit

error_already_bootstrapped:
    mov64 r0, 0x0D
    exit

; ── Helpers ───────────────────────────────────────────

; cmp32: r1=ptr_a, r2=ptr_b → r0=0 equal, r0=1 not-equal; clobbers r3,r4
cmp32:
    ldxdw r3, [r1 + 0]
    ldxdw r4, [r2 + 0]
    jne   r3, r4, cmp32_ne
    ldxdw r3, [r1 + 8]
    ldxdw r4, [r2 + 8]
    jne   r3, r4, cmp32_ne
    ldxdw r3, [r1 + 16]
    ldxdw r4, [r2 + 16]
    jne   r3, r4, cmp32_ne
    ldxdw r3, [r1 + 24]
    ldxdw r4, [r2 + 24]
    jne   r3, r4, cmp32_ne
    mov64 r0, 0
    exit
cmp32_ne:
    mov64 r0, 1
    exit

; copy32: r1=dst, r2=src; clobbers r3
copy32:
    ldxdw r3, [r2 + 0]
    stxdw [r1 + 0], r3
    ldxdw r3, [r2 + 8]
    stxdw [r1 + 8], r3
    ldxdw r3, [r2 + 16]
    stxdw [r1 + 16], r3
    ldxdw r3, [r2 + 24]
    stxdw [r1 + 24], r3
    exit

; fill_meta: r1=dst, r2=acct_ptr, r3=is_writable, r4=is_signer
fill_meta:
    add64 r2, ACCT_KEY
    stxdw [r1 + 0], r2
    stxb  [r1 + 8], r3
    stxb  [r1 + 9], r4
    exit

; fill_acct_info: r1=dst, r2=acct_ptr, r3=next_acct_ptr, r4=is_signer, r5=is_writable
fill_acct_info:
    mov64 r0, r2
    add64 r0, ACCT_KEY
    stxdw [r1 + 0], r0             ; key ptr
    mov64 r0, r2
    add64 r0, ACCT_LAMPORTS
    stxdw [r1 + 8], r0             ; lamports ptr
    ldxdw r0, [r2 + ACCT_DLEN]
    stxdw [r1 + 16], r0            ; data_len
    mov64 r0, r2
    add64 r0, ACCT_DATA
    stxdw [r1 + 24], r0            ; data ptr
    mov64 r0, r2
    add64 r0, ACCT_OWNER
    stxdw [r1 + 32], r0            ; owner ptr
    ldxdw r0, [r3 - 8]             ; rent_epoch = *(next_acct_ptr - 8)
    stxdw [r1 + 40], r0
    stxb  [r1 + 48], r4            ; is_signer
    stxb  [r1 + 49], r5            ; is_writable
    mov64 r0, 0
    stxb  [r1 + 50], r0            ; is_executable
    exit
