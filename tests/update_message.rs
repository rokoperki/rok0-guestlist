use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::{AccountMeta, Instruction, InstructionError},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::{Transaction, TransactionError},
};

// ── Error codes ───────────────────────────────────────────────────────
const ERR_WRONG_ACCT_COUNT: u32 = 0x02;
const ERR_NOT_SIGNER: u32 = 0x03;
const ERR_INVALID_PDA: u32 = 0x04;
const ERR_WRONG_OWNER: u32 = 0x06;
const ERR_WRONG_SIZE: u32 = 0x07;
const ERR_AUTHORITY_MISMATCH: u32 = 0x08;

// ── Offsets ───────────────────────────────────────────────────────────
const OS_BUMP: usize = 0x45;
const OS_MSG_LEN: usize = 0x46;
const OS_MESSAGE: usize = 0x48;
const OS_HEADER: usize = 0x48;

// ── Helpers ───────────────────────────────────────────────────────────

fn program_id() -> Pubkey {
    let raw = std::fs::read("deploy/rok0_guestbook-keypair.json").unwrap();
    let s = String::from_utf8(raw).unwrap();
    let nums: Vec<u8> = s.trim().trim_start_matches('[').trim_end_matches(']')
        .split(',').map(|n| n.trim().parse::<u8>().unwrap()).collect();
    Pubkey::from(<[u8; 32]>::try_from(&nums[32..64]).unwrap())
}

fn setup() -> (LiteSVM, Pubkey) {
    let mut svm = LiteSVM::new();
    let program_id = program_id();
    svm.add_program_from_file(program_id, "deploy/rok0_guestbook.so").unwrap();
    (svm, program_id)
}

fn custom_err(code: u32) -> TransactionError {
    TransactionError::InstructionError(0, InstructionError::Custom(code))
}

fn pda_of(wallet: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"overseer", &wallet.to_bytes()], program_id)
}

fn register(svm: &mut LiteSVM, program_id: Pubkey, authority: &Keypair, msg: &[u8]) -> (Pubkey, u8) {
    let (pda, bump) = pda_of(&authority.pubkey(), &program_id);
    let mut codename = [0u8; 16];
    codename[..6].copy_from_slice(b"CASPER");
    let mut d = vec![0u8];
    d.extend_from_slice(&codename);
    d.push(bump);
    d.extend_from_slice(&2_000_000u64.to_le_bytes());
    d.extend_from_slice(&(msg.len() as u16).to_le_bytes());
    d.extend_from_slice(msg);
    let ix = Instruction::new_with_bytes(
        program_id, &d,
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[authority], svm.latest_blockhash(),
    );
    svm.send_transaction(tx).expect("register failed");
    (pda, bump)
}

fn update_message_ix(program_id: Pubkey, authority: Pubkey, pda: Pubkey, new_msg: &[u8]) -> Instruction {
    let mut d = vec![5u8]; // disc = update_message
    d.extend_from_slice(&(new_msg.len() as u16).to_le_bytes());
    d.extend_from_slice(new_msg);
    Instruction::new_with_bytes(
        program_id, &d,
        vec![
            AccountMeta::new(authority, true),
            AccountMeta::new(pda, false),
        ],
    )
}

fn pda_account(program_id: Pubkey, authority: &Pubkey, bump: u8) -> Account {
    let mut data = vec![0u8; OS_HEADER];
    data[0x00..0x20].copy_from_slice(&authority.to_bytes());
    data[OS_BUMP] = bump;
    Account { lamports: 2_000_000, data, owner: program_id, executable: false, rent_epoch: u64::MAX }
}

fn print_logs(label: &str, result: &Result<litesvm::types::TransactionMetadata, litesvm::types::FailedTransactionMetadata>) {
    let logs = match result { Ok(m) => &m.logs, Err(e) => &e.meta.logs };
    println!("[{}]", label);
    for log in logs { println!("  {}", log); }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[test]
fn test_update_message_same_length() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let original = b"hello world";
    let (pda, _) = register(&mut svm, program_id, &authority, original);

    let new_msg = b"goodbye wrld";
    let ix = update_message_ix(program_id, authority.pubkey(), pda, new_msg);
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("update_same_length", &result);
    result.unwrap();

    let acct = svm.get_account(&pda).unwrap();
    assert_eq!(acct.data.len(), OS_MESSAGE + new_msg.len());
    let stored_len = u16::from_le_bytes([acct.data[OS_MSG_LEN], acct.data[OS_MSG_LEN + 1]]);
    assert_eq!(stored_len as usize, new_msg.len());
    assert_eq!(&acct.data[OS_MESSAGE..OS_MESSAGE + new_msg.len()], new_msg.as_slice());
}

#[test]
fn test_update_message_grow() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority, b"hi");

    let new_msg = b"this is a much longer message now";
    let ix = update_message_ix(program_id, authority.pubkey(), pda, new_msg);
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("update_grow", &result);
    result.unwrap();

    let acct = svm.get_account(&pda).unwrap();
    assert_eq!(acct.data.len(), OS_MESSAGE + new_msg.len());
    let stored_len = u16::from_le_bytes([acct.data[OS_MSG_LEN], acct.data[OS_MSG_LEN + 1]]);
    assert_eq!(stored_len as usize, new_msg.len());
    assert_eq!(&acct.data[OS_MESSAGE..OS_MESSAGE + new_msg.len()], new_msg.as_slice());
}

#[test]
fn test_update_message_shrink() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority, b"this is a long original message");

    let new_msg = b"short";
    let ix = update_message_ix(program_id, authority.pubkey(), pda, new_msg);
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("update_shrink", &result);
    result.unwrap();

    let acct = svm.get_account(&pda).unwrap();
    assert_eq!(acct.data.len(), OS_MESSAGE + new_msg.len());
    let stored_len = u16::from_le_bytes([acct.data[OS_MSG_LEN], acct.data[OS_MSG_LEN + 1]]);
    assert_eq!(stored_len as usize, new_msg.len());
    assert_eq!(&acct.data[OS_MESSAGE..OS_MESSAGE + new_msg.len()], new_msg.as_slice());
}

#[test]
fn test_update_message_clear() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority, b"some message");

    // update to empty message
    let ix = update_message_ix(program_id, authority.pubkey(), pda, b"");
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    svm.send_transaction(tx).unwrap();

    let acct = svm.get_account(&pda).unwrap();
    assert_eq!(acct.data.len(), OS_MESSAGE); // 72 bytes, no message
    let stored_len = u16::from_le_bytes([acct.data[OS_MSG_LEN], acct.data[OS_MSG_LEN + 1]]);
    assert_eq!(stored_len, 0);
}

#[test]
fn test_update_message_wrong_accounts_number() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id, &[5u8, 0, 0],
        vec![AccountMeta::new(authority.pubkey(), true)],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_WRONG_ACCT_COUNT));
}

#[test]
fn test_update_message_not_signer() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    let payer = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority, b"msg");

    let mut d = vec![5u8, 3, 0];
    d.extend_from_slice(b"new");
    let ix = Instruction::new_with_bytes(
        program_id, &d,
        vec![
            AccountMeta::new(authority.pubkey(), false), // NOT signer
            AccountMeta::new(pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&payer.pubkey()), &[&payer], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_NOT_SIGNER));
}

#[test]
fn test_update_message_wrong_owner() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = pda_of(&authority.pubkey(), &program_id);
    let mut acct = pda_account(program_id, &authority.pubkey(), bump);
    acct.owner = Pubkey::new_unique();
    svm.set_account(pda, acct).unwrap();

    let ix = update_message_ix(program_id, authority.pubkey(), pda, b"new");
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_WRONG_OWNER));
}

#[test]
fn test_update_message_authority_mismatch() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = pda_of(&authority.pubkey(), &program_id);
    // PDA stores a different authority
    svm.set_account(pda, pda_account(program_id, &Pubkey::new_unique(), bump)).unwrap();

    let ix = update_message_ix(program_id, authority.pubkey(), pda, b"new");
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_AUTHORITY_MISMATCH));
}

#[test]
fn test_update_message_invalid_pda() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (_, bump) = pda_of(&authority.pubkey(), &program_id);
    let fake_pda = Pubkey::new_unique();
    svm.set_account(fake_pda, pda_account(program_id, &authority.pubkey(), bump)).unwrap();

    let ix = update_message_ix(program_id, authority.pubkey(), fake_pda, b"new");
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_INVALID_PDA));
}

#[test]
fn test_update_message_too_long() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority, b"original");

    // msg_len = 701 > MSG_MAX (700)
    let mut d = vec![5u8];
    d.extend_from_slice(&701u16.to_le_bytes());
    d.extend_from_slice(&vec![b'x'; 701]);
    let ix = Instruction::new_with_bytes(
        program_id, &d,
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    // error_invalid_ix (0x01) — msg_len > MSG_MAX
    assert_eq!(svm.send_transaction(tx).unwrap_err().err,
        TransactionError::InstructionError(0, InstructionError::Custom(0x01)));
}
