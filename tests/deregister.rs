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
const ERR_NOT_COMMANDER: u32 = 0x09;

// ── Account data offsets ──────────────────────────────────────────────
const OS_CLEARANCE: usize = 0x44;
const OS_BUMP: usize = 0x45;
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

fn register(svm: &mut LiteSVM, program_id: Pubkey, authority: &Keypair) -> (Pubkey, u8) {
    let (pda, bump) = pda_of(&authority.pubkey(), &program_id);
    let mut codename = [0u8; 16];
    codename[..6].copy_from_slice(b"CASPER");
    let mut d = vec![0u8];
    d.extend_from_slice(&codename);
    d.push(bump);
    d.extend_from_slice(&2_000_000u64.to_le_bytes());
    d.extend_from_slice(&0u16.to_le_bytes());
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

fn set_clearance(svm: &mut LiteSVM, pda: Pubkey, clearance: u8) {
    let mut acct = svm.get_account(&pda).unwrap();
    acct.data[OS_CLEARANCE] = clearance;
    svm.set_account(pda, acct).unwrap();
}

fn pda_account(program_id: Pubkey, authority: &Pubkey, bump: u8, clearance: u8) -> Account {
    let mut data = vec![0u8; OS_HEADER];
    data[0x00..0x20].copy_from_slice(&authority.to_bytes());
    data[OS_BUMP] = bump;
    data[OS_CLEARANCE] = clearance;
    Account { lamports: 2_000_000, data, owner: program_id, executable: false, rent_epoch: u64::MAX }
}

fn print_logs(label: &str, result: &Result<litesvm::types::TransactionMetadata, litesvm::types::FailedTransactionMetadata>) {
    let logs = match result { Ok(m) => &m.logs, Err(e) => &e.meta.logs };
    println!("[{}]", label);
    for log in logs { println!("  {}", log); }
}

// ── Tests: Case A (self-deregister) ──────────────────────────────────

#[test]
fn test_deregister_self_success() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority);
    let pda_lamports = svm.get_account(&pda).unwrap().lamports;
    let auth_lamports_before = svm.get_account(&authority.pubkey()).unwrap().lamports;

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("deregister_self_success", &result);
    result.unwrap();

    // pda closed: lamports = 0 or account gone
    let pda_after = svm.get_account(&pda);
    assert!(
        pda_after.is_none() || pda_after.unwrap().lamports == 0,
        "pda should be closed"
    );

    // authority recovered the lamports (minus tx fees)
    let auth_lamports_after = svm.get_account(&authority.pubkey()).unwrap().lamports;
    assert!(
        auth_lamports_after > auth_lamports_before,
        "authority should recover pda lamports"
    );
    // recovered amount should be close to pda_lamports
    let recovered = auth_lamports_after - auth_lamports_before;
    assert!(recovered > pda_lamports / 2, "authority recovered most of pda lamports");
}

#[test]
fn test_deregister_self_wrong_accounts_number() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    // 1 account — neither 2 nor 3
    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![AccountMeta::new(authority.pubkey(), true)],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_WRONG_ACCT_COUNT));
}

#[test]
fn test_deregister_self_not_signer() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    let payer = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = register(&mut svm, program_id, &authority);

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
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
fn test_deregister_self_wrong_owner() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = pda_of(&authority.pubkey(), &program_id);
    let mut acct = pda_account(program_id, &authority.pubkey(), bump, 0);
    acct.owner = Pubkey::new_unique(); // wrong owner
    svm.set_account(pda, acct).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_WRONG_OWNER));
}

#[test]
fn test_deregister_self_wrong_size() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, _) = pda_of(&authority.pubkey(), &program_id);
    svm.set_account(pda, Account {
        lamports: 2_000_000, data: vec![0u8; 10], // too small
        owner: program_id, executable: false, rent_epoch: u64::MAX,
    }).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_WRONG_SIZE));
}

#[test]
fn test_deregister_self_authority_mismatch() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (pda, bump) = pda_of(&authority.pubkey(), &program_id);
    // PDA stores a different authority in its data
    let different_auth = Pubkey::new_unique();
    svm.set_account(pda, pda_account(program_id, &different_auth, bump, 0)).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_AUTHORITY_MISMATCH));
}

#[test]
fn test_deregister_self_invalid_pda() {
    let (mut svm, program_id) = setup();
    let authority = Keypair::new();
    svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();

    let (_, bump) = pda_of(&authority.pubkey(), &program_id);
    // Pass a wrong address as pda but with correct authority+bump in data
    let fake_pda = Pubkey::new_unique();
    svm.set_account(fake_pda, pda_account(program_id, &authority.pubkey(), bump, 0)).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(fake_pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&authority.pubkey()), &[&authority], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_INVALID_PDA));
}

// ── Tests: Case B (commander-deregister) ─────────────────────────────

#[test]
fn test_deregister_commander_success() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, _) = register(&mut svm, program_id, &commander);
    let (target_pda, _)    = register(&mut svm, program_id, &target);
    set_clearance(&mut svm, commander_pda, 2); // elevate to COMMANDER

    let target_lamports = svm.get_account(&target_pda).unwrap().lamports;
    let cmd_lamports_before = svm.get_account(&commander.pubkey()).unwrap().lamports;

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![
            AccountMeta::new(commander.pubkey(), true),
            AccountMeta::new_readonly(commander_pda, false),
            AccountMeta::new(target_pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("deregister_commander_success", &result);
    result.unwrap();

    // target_pda closed
    let tgt_after = svm.get_account(&target_pda);
    assert!(
        tgt_after.is_none() || tgt_after.unwrap().lamports == 0,
        "target_pda should be closed"
    );

    // commander recovered lamports
    let cmd_lamports_after = svm.get_account(&commander.pubkey()).unwrap().lamports;
    assert!(cmd_lamports_after > cmd_lamports_before);
    let recovered = cmd_lamports_after - cmd_lamports_before;
    assert!(recovered > target_lamports / 2);
}

#[test]
fn test_deregister_commander_not_commander() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, _) = register(&mut svm, program_id, &commander);
    let (target_pda, _)    = register(&mut svm, program_id, &target);
    // commander stays at OPERATIVE (clearance=0)

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![
            AccountMeta::new(commander.pubkey(), true),
            AccountMeta::new_readonly(commander_pda, false),
            AccountMeta::new(target_pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("not_commander", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_NOT_COMMANDER));
}

#[test]
fn test_deregister_commander_wrong_owner() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, cmd_bump) = pda_of(&commander.pubkey(), &program_id);
    let (target_pda, _) = register(&mut svm, program_id, &target);

    // commander_pda with wrong owner
    let mut acct = pda_account(program_id, &commander.pubkey(), cmd_bump, 2);
    acct.owner = Pubkey::new_unique();
    svm.set_account(commander_pda, acct).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![
            AccountMeta::new(commander.pubkey(), true),
            AccountMeta::new_readonly(commander_pda, false),
            AccountMeta::new(target_pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_WRONG_OWNER));
}

#[test]
fn test_deregister_commander_invalid_target_pda() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, _) = register(&mut svm, program_id, &commander);
    let (_, target_bump)   = pda_of(&target.pubkey(), &program_id);
    set_clearance(&mut svm, commander_pda, 2);

    // fake target_pda at wrong address
    let fake_pda = Pubkey::new_unique();
    svm.set_account(fake_pda, pda_account(program_id, &target.pubkey(), target_bump, 0)).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id, &[3u8],
        vec![
            AccountMeta::new(commander.pubkey(), true),
            AccountMeta::new_readonly(commander_pda, false),
            AccountMeta::new(fake_pda, false),
        ],
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_INVALID_PDA));
}
