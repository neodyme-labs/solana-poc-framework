use std::{
    cell::RefCell,
    convert::TryInto,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use once_cell::sync::OnceCell;
use poc_framework::*;
use solana_program::pubkey;
use solana_runtime::{
    bank::TxComputeMeter,
    log_collector::LogCollector,
    message_processor::{Executors, MessageProcessor},
    rent_collector::RentCollector,
};
use solana_sdk::{
    account::{Account, AccountSharedData, ReadableAccount},
    feature_set::FeatureSet,
    instruction::Instruction,
    message::Message,
    process_instruction::{BpfComputeBudget, ProcessInstructionWithContext},
    program_pack::Pack,
    pubkey::Pubkey,
    sysvar,
    sysvar_cache::SysvarCache,
    transaction::{Transaction, TransactionError},
};

type SerializedTxExecution = (
    Transaction,
    Vec<Vec<(Pubkey, Account)>>,
    Vec<(Pubkey, Account)>,
    RentCollector,
);

const EXTRACT_ACCOUNTS_PROGRAM: Pubkey = pubkey!("Extract1111111111111111111111111111111111111");
static BUILTIN_PROGRAMS: OnceCell<Vec<(Pubkey, ProcessInstructionWithContext)>> = OnceCell::new();
static RENT_COLLECTOR: OnceCell<RentCollector> = OnceCell::new();

fn init_builtin_programs() {
    let mut env = LocalEnvironment::builder().build();
    env.bank().add_builtin(
        "extract_accounts",
        EXTRACT_ACCOUNTS_PROGRAM,
        |_id, _data, ctx| {
            let _ = BUILTIN_PROGRAMS.set(ctx.get_programs().to_vec());
            Ok(())
        },
    );
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
    accounts: &[(Pubkey, Rc<RefCell<AccountSharedData>>)],
) -> (Result<(), TransactionError>, Vec<String>) {
    let mut message_processor = MessageProcessor::default();
    for (pk, processor) in BUILTIN_PROGRAMS.get().unwrap() {
        message_processor.add_program(*pk, *processor);
    }

    let executors = Rc::new(RefCell::new(Executors::default()));
    let compute_meter = Rc::new(RefCell::new(TxComputeMeter::new(
        BpfComputeBudget::new().max_units + 10000000000000,
    )));
    let mut timings = Default::default();
    let mut sysvar_cache = SysvarCache::default();
    sysvar_cache.push_entry(
        sysvar::clock::id(),
        bincode::serialize(&sysvar::clock::Clock {
            slot: 119342570,
            epoch_start_timestamp: 1644004275 - 60 * 60 * 24,
            epoch: 276,
            leader_schedule_epoch: 276,
            unix_timestamp: 1644004275,
        })
        .unwrap(),
    );
    let log_collector = Rc::new(LogCollector::default());

    let res = message_processor.process_message(
        tx.message(),
        &loaders,
        &accounts,
        &RENT_COLLECTOR.get().unwrap(),
        Some(log_collector.clone()),
        executors,
        None,
        Arc::new(FeatureSet::all_enabled()),
        BpfComputeBudget::new(),
        compute_meter,
        &mut timings,
        &sysvar_cache,
    );

    (res, Rc::try_unwrap(log_collector).ok().unwrap().into())
}

fn print_tx_result(result: (Result<(), TransactionError>, Vec<String>)) {
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

    let path = PathBuf::from(todo!());
    let mut file = File::open(path).expect("open file");

    let execution: SerializedTxExecution =
        bincode::deserialize_from(&mut file).expect("deserialize");
    let (tx, loaders, accounts, rent_collector) = execution;
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
