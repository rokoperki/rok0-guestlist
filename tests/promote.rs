use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::{AccountMeta, Instruction, InstructionError},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::{Transaction, TransactionError},
    system_program,
};

// ── Error codes ───────────────────────────────────────────────────────
const ERR_WRONG_ACCT_COUNT: u32 = 0x02;
const ERR_NOT_SIGNER: u32 = 0x03;
const ERR_INVALID_PDA: u32 = 0x04;
const ERR_WRONG_OWNER: u32 = 0x06;
const ERR_WRONG_SIZE: u32 = 0x07;
const ERR_AUTHORITY_MISMATCH: u32 = 0x08;
const ERR_NOT_COMMANDER: u32 = 0x09;
const ERR_NOT_OPERATIVE: u32 = 0x0A;

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

fn promote_ix(
    program_id: Pubkey,
    commander: Pubkey,
    commander_pda: Pubkey,
    target_pda: Pubkey,
    target_wallet: &Pubkey,
) -> Instruction {
    let mut d = vec![2u8]; // disc = 2
    d.extend_from_slice(&target_wallet.to_bytes());
    Instruction::new_with_bytes(
        program_id, &d,
        vec![
            AccountMeta::new(commander, true),
            AccountMeta::new_readonly(commander_pda, false),
            AccountMeta::new(target_pda, false),
        ],
    )
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

// ── Tests ─────────────────────────────────────────────────────────────

#[test]
fn test_promote_success() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, _) = register(&mut svm, program_id, &commander);
    let (target_pda, _)    = register(&mut svm, program_id, &target);

    // Elevate commander to COMMANDER
    set_clearance(&mut svm, commander_pda, 2);

    let ix = promote_ix(program_id, commander.pubkey(), commander_pda, target_pda, &target.pubkey());
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("promote_success", &result);
    result.unwrap();

    let acct = svm.get_account(&target_pda).unwrap();
    assert_eq!(acct.data[OS_CLEARANCE], 1, "target should be OVERSEER");

    // Commander's clearance unchanged
    let cmd = svm.get_account(&commander_pda).unwrap();
    assert_eq!(cmd.data[OS_CLEARANCE], 2, "commander unchanged");
}

#[test]
fn test_promote_wrong_accounts_number() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();

    let ix = Instruction::new_with_bytes(
        program_id, &[2u8],
        vec![AccountMeta::new(commander.pubkey(), true)], // only 1
    );
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    assert_eq!(svm.send_transaction(tx).unwrap_err().err, custom_err(ERR_WRONG_ACCT_COUNT));
}

#[test]
fn test_promote_not_signer() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let payer = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, _) = register(&mut svm, program_id, &commander);
    let (target_pda, _)    = register(&mut svm, program_id, &target);

    let mut d = vec![2u8];
    d.extend_from_slice(&target.pubkey().to_bytes());
    let ix = Instruction::new_with_bytes(
        program_id, &d,
        vec![
            AccountMeta::new(commander.pubkey(), false), // NOT signer
            AccountMeta::new_readonly(commander_pda, false),
            AccountMeta::new(target_pda, false),
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
fn test_promote_not_commander() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, _) = register(&mut svm, program_id, &commander);
    let (target_pda, _)    = register(&mut svm, program_id, &target);
    // Commander stays at OPERATIVE (clearance=0) — not elevated

    let ix = promote_ix(program_id, commander.pubkey(), commander_pda, target_pda, &target.pubkey());
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("not_commander", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_NOT_COMMANDER));
}

#[test]
fn test_promote_not_operative() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, _) = register(&mut svm, program_id, &commander);
    let (target_pda, _)    = register(&mut svm, program_id, &target);
    set_clearance(&mut svm, commander_pda, 2); // commander = COMMANDER
    set_clearance(&mut svm, target_pda, 1);    // target already OVERSEER

    let ix = promote_ix(program_id, commander.pubkey(), commander_pda, target_pda, &target.pubkey());
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("not_operative", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_NOT_OPERATIVE));
}

#[test]
fn test_promote_wrong_target_wallet_in_ix() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, _) = register(&mut svm, program_id, &commander);
    let (target_pda, _)    = register(&mut svm, program_id, &target);
    set_clearance(&mut svm, commander_pda, 2);

    // Pass a wrong wallet in ix_data (not the target's actual key)
    let wrong_wallet = Pubkey::new_unique();
    let ix = promote_ix(program_id, commander.pubkey(), commander_pda, target_pda, &wrong_wallet);
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("wrong_target_wallet", &result);
    // target_pda.authority != wrong_wallet → authority_mismatch
    assert_eq!(result.unwrap_err().err, custom_err(ERR_AUTHORITY_MISMATCH));
}

#[test]
fn test_promote_invalid_target_pda() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, _) = register(&mut svm, program_id, &commander);
    let (_, target_bump)   = pda_of(&target.pubkey(), &program_id);
    set_clearance(&mut svm, commander_pda, 2);

    // Craft a fake target_pda at a wrong address but with target's authority+bump stored
    let fake_pda = Pubkey::new_unique();
    svm.set_account(fake_pda, pda_account(program_id, &target.pubkey(), target_bump, 0)).unwrap();

    let ix = promote_ix(program_id, commander.pubkey(), commander_pda, fake_pda, &target.pubkey());
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("invalid_target_pda", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_INVALID_PDA));
}

#[test]
fn test_promote_commander_wrong_owner() {
    let (mut svm, program_id) = setup();
    let commander = Keypair::new();
    let target = Keypair::new();
    svm.airdrop(&commander.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&target.pubkey(), 10_000_000_000).unwrap();

    let (commander_pda, cmd_bump) = pda_of(&commander.pubkey(), &program_id);
    let (target_pda, _) = register(&mut svm, program_id, &target);

    // Commander PDA exists but with wrong owner
    svm.set_account(commander_pda, Account {
        lamports: 2_000_000,
        data: { let mut d = pda_account(program_id, &commander.pubkey(), cmd_bump, 2).data; d[OS_CLEARANCE] = 2; d },
        owner: Pubkey::new_unique(), // wrong!
        executable: false,
        rent_epoch: u64::MAX,
    }).unwrap();

    let ix = promote_ix(program_id, commander.pubkey(), commander_pda, target_pda, &target.pubkey());
    let tx = Transaction::new_signed_with_payer(
        &[ix], Some(&commander.pubkey()), &[&commander], svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    print_logs("commander_wrong_owner", &result);
    assert_eq!(result.unwrap_err().err, custom_err(ERR_WRONG_OWNER));
}
