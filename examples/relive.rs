use std::{
    cell::RefCell,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
use std::convert::TryFrom;

use once_cell::sync::OnceCell;
use solana_program::hash::Hash;
use poc_framework::*;
use solana_program::message::SanitizedMessage;
use solana_program::pubkey;
use solana_program::rent::Rent;
use solana_program_runtime::{
    compute_budget::ComputeBudget,
    invoke_context::{ComputeMeter, Executors, ProcessInstructionWithContext},
    log_collector::LogCollector,
    sysvar_cache::SysvarCache,
};
use solana_program_runtime::invoke_context::BuiltinProgram;
use solana_runtime::{message_processor::MessageProcessor, rent_collector::RentCollector};
use solana_runtime::message_processor::ProcessedMessageInfo;
use solana_sdk::{
    account::{Account, AccountSharedData, ReadableAccount},
    feature_set::FeatureSet,
    instruction::Instruction,
    message::Message,
    program_pack::Pack,
    pubkey::Pubkey,
    sysvar,
    transaction::{Transaction, TransactionError},
};
use solana_sdk::transaction_context::{TransactionAccount, TransactionContext};

type SerializedTxExecution = (
    Transaction,
    Vec<Vec<(Pubkey, Account)>>,
    Vec<(Pubkey, Account)>,
    RentCollector,
);

const EXTRACT_ACCOUNTS_PROGRAM: Pubkey = pubkey!("Extract1111111111111111111111111111111111111");
static BUILTIN_PROGRAMS: OnceCell<Vec<BuiltinProgram>> = OnceCell::new();
static RENT_COLLECTOR: OnceCell<RentCollector> = OnceCell::new();

fn init_builtin_programs() {
    let mut env = LocalEnvironment::builder().build();
    env.execute_as_transaction(
        &[Instruction {
            program_id: EXTRACT_ACCOUNTS_PROGRAM,
            accounts: vec![],
            data: vec![],
        }],
        &[],
    )
    .assert_success();
}

fn update_ix_sysvar(accs: &[(Pubkey, Rc<RefCell<AccountSharedData>>)], message: &Message) {
    let mut ix_bytes = message.serialize_instructions(true);
    ix_bytes.extend_from_slice(&01u16.to_le_bytes());

    for (pk, acc) in accs {
        if sysvar::instructions::check_id(pk) {
            acc.borrow_mut().set_data(ix_bytes.clone());
        }
    }
}

fn execute(
    tx: &Transaction,
    loaders: &[Vec<(Pubkey, Rc<RefCell<AccountSharedData>>)>],
    accounts: Vec<TransactionAccount>,
) -> (Result<ProcessedMessageInfo, TransactionError>, Vec<String>) {
    let executors = Rc::new(RefCell::new(Executors::default()));
    let compute_meter = ComputeMeter::new_ref(10000000000000);
    let mut timings = Default::default();
    let mut sysvar_cache = SysvarCache::default();
    sysvar_cache.set_clock(sysvar::clock::Clock {
        slot: 119342570,
        epoch_start_timestamp: 1644004275 - 60 * 60 * 24,
        epoch: 276,
        leader_schedule_epoch: 276,
        unix_timestamp: 1644004275,
    });
    let log_collector = Rc::new(RefCell::new(LogCollector::default()));

    let mut context = TransactionContext::new(accounts, 1, 1, 10000);

    let res = MessageProcessor::process_message(
        BUILTIN_PROGRAMS.get().unwrap(),
        &SanitizedMessage::try_from(tx.message().clone()).unwrap(),
        &[vec![0]],
        &mut context,
        Rent::default(),
        Some(Rc::clone(&log_collector)),
        executors,
        Arc::new(FeatureSet::all_enabled()),
        ComputeBudget::new(10000000),
        &mut timings,
        &sysvar_cache,
        Hash::default(),
        0,
        0,
        &mut 0,
    );

    (res, Rc::try_unwrap(log_collector).ok().unwrap().take().into())
}

fn print_tx_result(result: (Result<ProcessedMessageInfo, TransactionError>, Vec<String>)) {
    let (status, logs) = result;
    for log in logs {
        println!("{}", log);
    }
    println!("status: {:?}", status);
}

fn save_account<T: AsRef<Path>>(
    accs: &[(Pubkey, Rc<RefCell<AccountSharedData>>)],
    pubkey: Pubkey,
    path: T,
) {
    let mut out = File::create(path).expect("create file");
    let (_pk, acc) = accs.iter().find(|(pk, _)| *pk == pubkey).unwrap();
    out.write_all(acc.borrow().data()).expect("write");
}

fn get_token_acc(
    accs: &[(Pubkey, Rc<RefCell<AccountSharedData>>)],
    id: Pubkey,
) -> spl_token::state::Account {
    spl_token::state::Account::unpack(
        &accs
            .iter()
            .find(|(pk, _)| *pk == id)
            .unwrap()
            .1
            .borrow()
            .data(),
    )
    .expect("deser")
}

fn main() {
    init_builtin_programs();

    let path: PathBuf = todo!();
    let mut file = File::open(path).expect("open file");

    let execution: SerializedTxExecution =
        bincode::deserialize_from(&mut file).expect("deserialize");
    let (new_tx, loaders, accounts, rent_collector) = execution;
    RENT_COLLECTOR.set(rent_collector).unwrap();
    let loaders = loaders
        .into_iter()
        .map(|v| {
            v.into_iter()
                .map(|(pk, v)| (pk, Rc::new(RefCell::new(v.into()))))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let accounts = accounts
        .into_iter()
        .map(|(pk, v)| (pk, Rc::new(RefCell::new(v.into()))))
        .collect::<Vec<_>>();

    update_ix_sysvar(&accounts, new_tx.message());
    print_tx_result(execute(&new_tx, &loaders, &accounts));
}
