use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::{AccountMeta, Instruction, InstructionError},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::{Transaction, TransactionError},
};

// ── Error codes (must match rok0_guestbook.s) ─────────────────────────
const ERR_INVALID_IX: u32 = 0x01;
const ERR_WRONG_ACCT_COUNT: u32 = 0x02;
const ERR_NOT_SIGNER: u32 = 0x03;
const ERR_INVALID_PDA: u32 = 0x04;

// ── Account data offsets ──────────────────────────────────────────────
const OS_AUTHORITY: usize = 0x00;
const OS_CODENAME: usize = 0x20;
const OS_CLEARANCE: usize = 0x44;
const OS_BUMP: usize = 0x45;
const OS_MSG_LEN: usize = 0x46;
const OS_MESSAGE: usize = 0x48;

// ── Helpers ───────────────────────────────────────────────────────────

// Program ID is at bytes 32..64 of the keypair file (public key portion).
fn program_id() -> Pubkey {
    let raw = std::fs::read("deploy/rok0_guestbook-keypair.json")
        .expect("keypair not found — run solana-keygen first");
    let s = String::from_utf8(raw).unwrap();
    let nums: Vec<u8> = s
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|n| n.trim().parse::<u8>().unwrap())
        .collect();
    Pubkey::from(<[u8; 32]>::try_from(&nums[32..64]).unwrap())
}

fn setup() -> (LiteSVM, Pubkey) {
    let mut svm = LiteSVM::new();
    let program_id = program_id();
    svm.add_program_from_file(program_id, "deploy/rok0_guestbook.so")
        .expect("failed to load rok0_guestbook.so — build it first");
    (svm, program_id)
}

fn custom_err(code: u32) -> TransactionError {
    TransactionError::InstructionError(0, InstructionError::Custom(code))
}

fn register_ix_data(codename: &[u8; 16], bump: u8, lamports: u64, msg: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8]; // disc = 0 (register)
    d.extend_from_slice(codename);
    d.push(bump);
    d.extend_from_slice(&lamports.to_le_bytes());
    d.extend_from_slice(&(msg.len() as u16).to_le_bytes());
    d.extend_from_slice(msg);
    d
}

fn print_logs(
    label: &str,
    result: &Result<
        litesvm::types::TransactionMetadata,
        litesvm::types::FailedTransactionMetadata,
    >,
) {
    let logs = match result {
        Ok(m) => &m.logs,
        Err(e) => &e.meta.logs,
    };
    println!("[{}]", label);
    for log in logs {
        println!("  {}", log);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[test]
fn test_register_success() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = Pubkey::find_program_address(
        &[b"overseer", &authority.pubkey().to_bytes()],
        &program_id,
    );

    let mut codename = [0u8; 16];
    codename[..6].copy_from_slice(b"CASPER");
    let msg = b"hello rokoperki";
    let lamports: u64 = 2_000_000;

    let ix = Instruction::new_with_bytes(
        program_id,
        &register_ix_data(&codename, bump, lamports, msg),
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&authority.pubkey()),
        &[&authority],
        svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("register_success", &result);
    result.unwrap();

    let acct = svm.get_account(&pda).unwrap();
    assert_eq!(acct.data.len(), OS_MESSAGE + msg.len(), "wrong account size");
    assert_eq!(&acct.data[OS_AUTHORITY..OS_AUTHORITY + 32], &authority.pubkey().to_bytes(), "authority");
    assert_eq!(&acct.data[OS_CODENAME..OS_CODENAME + 16], &codename, "codename");
    assert_eq!(acct.data[OS_CLEARANCE], 0, "clearance = OPERATIVE");
    assert_eq!(acct.data[OS_BUMP], bump, "bump");
    let stored_msg_len = u16::from_le_bytes([acct.data[OS_MSG_LEN], acct.data[OS_MSG_LEN + 1]]);
    assert_eq!(stored_msg_len as usize, msg.len(), "msg_len");
    assert_eq!(&acct.data[OS_MESSAGE..OS_MESSAGE + msg.len()], msg.as_slice(), "message");
}

#[test]
fn test_register_empty_message() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = Pubkey::find_program_address(
        &[b"overseer", &authority.pubkey().to_bytes()],
        &program_id,
    );
    let mut codename = [0u8; 16];
    codename[..4].copy_from_slice(b"MAGI");

    let ix = Instruction::new_with_bytes(
        program_id,
        &register_ix_data(&codename, bump, 2_000_000, b""),
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("register_empty_message", &result);
    result.unwrap();

    let acct = svm.get_account(&pda).unwrap();
    assert_eq!(acct.data.len(), OS_MESSAGE, "empty message → 72 bytes");
    let stored_msg_len = u16::from_le_bytes([acct.data[OS_MSG_LEN], acct.data[OS_MSG_LEN + 1]]);
    assert_eq!(stored_msg_len, 0);
}

#[test]
fn test_register_wrong_accounts_number() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id,
        &[0u8],
        vec![AccountMeta::new(authority.pubkey(), true)], // only 1 account
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("wrong_accounts_number", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_WRONG_ACCT_COUNT));
}

#[test]
fn test_register_not_signer() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = Pubkey::find_program_address(
        &[b"overseer", &authority.pubkey().to_bytes()],
        &program_id,
    );
    let mut codename = [0u8; 16];
    codename[..6].copy_from_slice(b"CASPER");

    let ix = Instruction::new_with_bytes(
        program_id,
        &register_ix_data(&codename, bump, 2_000_000, b"hello"),
        vec![
            AccountMeta::new(authority.pubkey(), false), // NOT signer
            AccountMeta::new(pda, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&payer.pubkey()), &[&payer], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("not_signer", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_NOT_SIGNER));
}

#[test]
fn test_register_invalid_pda() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = Pubkey::find_program_address(
        &[b"overseer", &authority.pubkey().to_bytes()],
        &program_id,
    );
    let mut codename = [0u8; 16];
    codename[..6].copy_from_slice(b"CASPER");

    // Pass wrong bump → derived PDA won't match acct1.key
    let bad_bump = bump.wrapping_add(1);
    let ix = Instruction::new_with_bytes(
        program_id,
        &register_ix_data(&codename, bad_bump, 2_000_000, b"hello"),
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("invalid_pda", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_INVALID_PDA));
}

#[test]
fn test_register_ix_too_short() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    let pda = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id,
        &[0u8], // only discriminator
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda.pubkey(), false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("ix_too_short", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_INVALID_IX));
}
