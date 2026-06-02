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

// ── Account data offsets ──────────────────────────────────────────────
const OS_VISITS: usize = 0x40;
const OS_BUMP: usize = 0x45;
const OS_HEADER: usize = 0x48;

// ── Helpers ───────────────────────────────────────────────────────────

fn program_id() -> Pubkey {
    let raw = std::fs::read("deploy/rok0_guestbook-keypair.json").unwrap();
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
    svm.add_program_from_file(program_id, "deploy/rok0_guestbook.so").unwrap();
    (svm, program_id)
}

fn custom_err(code: u32) -> TransactionError {
    TransactionError::InstructionError(0, InstructionError::Custom(code))
}

// Send register then return (pda, bump)
fn register(svm: &mut LiteSVM, program_id: Pubkey, authority: &Keypair) -> (Pubkey, u8) {
    let (pda, bump) = Pubkey::find_program_address(
        &[b"overseer", &authority.pubkey().to_bytes()],
        &program_id,
    );
    let mut codename = [0u8; 16];
    codename[..6].copy_from_slice(b"CASPER");
    let mut ix_data = vec![0u8];
    ix_data.extend_from_slice(&codename);
    ix_data.push(bump);
    ix_data.extend_from_slice(&2_000_000u64.to_le_bytes());
    ix_data.extend_from_slice(&0u16.to_le_bytes()); // msg_len = 0
    let ix = Instruction::new_with_bytes(
        program_id,
        &ix_data,
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

fn heartbeat_ix(program_id: Pubkey, authority: Pubkey, pda: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        program_id,
        &[1u8], // disc = 1
        vec![
            AccountMeta::new(authority, true),
            AccountMeta::new(pda, false),
        ],
    )
}

// Manually craft a PDA account for error-path tests
fn pda_account(program_id: Pubkey, authority: &Pubkey, bump: u8) -> Account {
    let mut data = vec![0u8; OS_HEADER];
    data[0x00..0x20].copy_from_slice(&authority.to_bytes());
    data[OS_BUMP] = bump;
    Account {
        lamports: 2_000_000,
        data,
        owner: program_id,
        executable: false,
        rent_epoch: u64::MAX,
    }
}

fn print_logs(
    label: &str,
    result: &Result<litesvm::types::TransactionMetadata, litesvm::types::FailedTransactionMetadata>,
) {
    let logs = match result { Ok(m) => &m.logs, Err(e) => &e.meta.logs };
    println!("[{}]", label);
    for log in logs { println!("  {}", log); }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[test]
fn test_heartbeat_success() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority);

    let tx = Transaction::new_signed_with_payer(
        &[heartbeat_ix(program_id, authority.pubkey(), pda)],
        Some(&authority.pubkey()),
        &[&authority],
        svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("heartbeat_success", &result);
    result.unwrap();

    let acct = svm.get_account(&pda).unwrap();
    let visits = u32::from_le_bytes(acct.data[OS_VISITS..OS_VISITS + 4].try_into().unwrap());
    assert_eq!(visits, 1, "visits should be 1 after first heartbeat");
}

#[test]
fn test_heartbeat_increments() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority);

    // Each iteration uses a fresh fee-payer keypair so the transaction is
    // unique (different payer pubkey → different message → different signature).
    for expected in [1u32, 2, 3] {
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000).unwrap();
        let tx = Transaction::new_signed_with_payer(
            &[heartbeat_ix(program_id, authority.pubkey(), pda)],
            Some(&payer.pubkey()),
            &[&payer, &authority], // payer covers fee; authority signs the ix
            svm.latest_blockhash(),
        );
        svm.send_transaction(tx).unwrap();
        let acct = svm.get_account(&pda).unwrap();
        let visits = u32::from_le_bytes(acct.data[OS_VISITS..OS_VISITS + 4].try_into().unwrap());
        assert_eq!(visits, expected);
    }
}

#[test]
fn test_heartbeat_wrong_accounts_number() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id,
        &[1u8],
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
fn test_heartbeat_not_signer() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    let payer = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority);

    let ix = Instruction::new_with_bytes(
        program_id,
        &[1u8],
        vec![
            AccountMeta::new(authority.pubkey(), false), // NOT signer
            AccountMeta::new(pda, false),
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
fn test_heartbeat_wrong_owner() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = Pubkey::find_program_address(
        &[b"overseer", &authority.pubkey().to_bytes()],
        &program_id,
    );
    // Set PDA with wrong owner
    let mut acct = pda_account(program_id, &authority.pubkey(), bump);
    acct.owner = Pubkey::new_unique();
    svm.set_account(pda, acct).unwrap();

    let tx = Transaction::new_signed_with_payer(
        &[heartbeat_ix(program_id, authority.pubkey(), pda)],
        Some(&authority.pubkey()),
        &[&authority],
        svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("wrong_owner", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_WRONG_OWNER));
}

#[test]
fn test_heartbeat_wrong_size() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = Pubkey::find_program_address(
        &[b"overseer", &authority.pubkey().to_bytes()],
        &program_id,
    );
    svm.set_account(pda, Account {
        lamports: 2_000_000,
        data: vec![0u8; 10], // too small (< 72)
        owner: program_id,
        executable: false,
        rent_epoch: u64::MAX,
    }).unwrap();

    let tx = Transaction::new_signed_with_payer(
        &[heartbeat_ix(program_id, authority.pubkey(), pda)],
        Some(&authority.pubkey()),
        &[&authority],
        svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("wrong_size", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_WRONG_SIZE));
}

#[test]
fn test_heartbeat_authority_mismatch() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = Pubkey::find_program_address(
        &[b"overseer", &authority.pubkey().to_bytes()],
        &program_id,
    );
    // Store a different authority in the account data
    let different_authority = Pubkey::new_unique();
    svm.set_account(pda, pda_account(program_id, &different_authority, bump)).unwrap();

    let tx = Transaction::new_signed_with_payer(
        &[heartbeat_ix(program_id, authority.pubkey(), pda)],
        Some(&authority.pubkey()),
        &[&authority],
        svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("authority_mismatch", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_AUTHORITY_MISMATCH));
}

#[test]
fn test_heartbeat_invalid_pda() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (_, bump) = Pubkey::find_program_address(
        &[b"overseer", &authority.pubkey().to_bytes()],
        &program_id,
    );
    // Pass a wrong address as acct1 — but with valid-looking data (correct authority+bump)
    let wrong_pda = Pubkey::new_unique();
    svm.set_account(wrong_pda, pda_account(program_id, &authority.pubkey(), bump)).unwrap();

    let tx = Transaction::new_signed_with_payer(
        &[heartbeat_ix(program_id, authority.pubkey(), wrong_pda)],
        Some(&authority.pubkey()),
        &[&authority],
        svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("invalid_pda", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_INVALID_PDA));
}
