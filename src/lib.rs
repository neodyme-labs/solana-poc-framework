use crate::solana_sdk::clock::UnixTimestamp;
use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
    time::{SystemTime, UNIX_EPOCH},
};

use borsh::{BorshDeserialize, BorshSerialize};
use bpf_loader_upgradeable::UpgradeableLoaderState;
use itertools::izip;
use rand::{prelude::StdRng, rngs::OsRng, SeedableRng};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use solana_accounts_db::{
    accounts_db::AccountShrinkThreshold,
    accounts_index::AccountSecondaryIndexes,
    transaction_results::{TransactionExecutionResult, TransactionResults},
};
use solana_cli_output::display::println_transaction;
use solana_client::{rpc_client::RpcClient, rpc_config::RpcTransactionConfig};
use solana_program::{
    bpf_loader, bpf_loader_upgradeable,
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    loader_instruction,
    message::Message,
    program_option::COption,
    program_pack::Pack,
    pubkey::Pubkey,
    system_instruction, system_program,
    sysvar::{self, rent},
};
use solana_runtime::{
    bank::{Bank, TransactionBalancesSet},
    genesis_utils,
    runtime_config::RuntimeConfig,
};
use solana_sdk::{
    account::{Account, AccountSharedData},
    commitment_config::CommitmentConfig,
    feature_set,
    genesis_config::GenesisConfig,
    packet::{self, PACKET_DATA_SIZE},
    signature::{Keypair, Signature, Signer},
    system_transaction,
    transaction::{Transaction, VersionedTransaction},
};
use solana_transaction_status::{
    ConfirmedTransactionWithStatusMeta, EncodedConfirmedTransactionWithStatusMeta,
    InnerInstructions, TransactionStatusMeta, TransactionWithStatusMeta, UiTransactionEncoding,
    VersionedTransactionWithStatusMeta,
};
use spl_associated_token_account::get_associated_token_address;

pub use bincode;
pub use borsh;
pub use serde;
pub use solana_client;
pub use solana_program;
pub use solana_sdk;
pub use solana_transaction_status;
pub use spl_associated_token_account;
pub use spl_memo;
pub use spl_token;
pub use spl_token_2022;

mod keys;
mod programs;

/// A generic Environment trait. Provides the possibility of writing generic exploits that work both remote and local, for easy debugging.
pub trait Environment {
    /// Returns the keypair used to pay for all transactions. All transaction fees and rent costs are payed for by this keypair.
    fn payer(&self) -> Keypair;
    /// Executes the batch of transactions in the right order and waits for them to be confirmed. The execution results are returned.
    fn execute_transaction<T>(&mut self, tx: T) -> EncodedConfirmedTransactionWithStatusMeta
    where
        VersionedTransaction: From<T>;
    /// Fetch a recent blockhash, for construction of transactions.
    #[deprecated(since = "0.2.0", note = "Please use `get_latest_blockhash()` instead")]
    fn get_recent_blockhash(&self) -> Hash {
        self.get_latest_blockhash()
    }
    /// Fetch the latest blockhash, for construction of transactions.
    fn get_latest_blockhash(&self) -> Hash;
    /// Fetch the amount of lamports needed for an account of the given size to be rent excempt.
    fn get_rent_excemption(&self, data: usize) -> u64;
    /// Fetch an account. None if the account does not exist.
    fn get_account(&self, pubkey: Pubkey) -> Option<Account>;

    /// Assemble the given instructions into a transaction and sign it. All transactions constructed by this method are signed and payed for by the payer.
    fn tx_with_instructions(
        &self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> Transaction {
        let payer = self.payer();
        let mut signer_vec = vec![&payer];
        signer_vec.extend_from_slice(signers);

        let message = Message::new(instructions, Some(&self.payer().pubkey()));
        let num_sigs: usize = message.header.num_required_signatures.into();
        let required_sigs = message.account_keys[..num_sigs]
            .into_iter()
            .copied()
            .collect::<HashSet<_>>();
        let provided_sigs = signer_vec
            .iter()
            .map(|x| x.pubkey())
            .collect::<HashSet<_>>();

        for key in required_sigs.difference(&provided_sigs) {
            println!("missing signature from {}", key.to_string());
        }

        for key in provided_sigs.difference(&required_sigs) {
            println!("unnecessary signature from {}", key.to_string());
        }

        Transaction::new(&signer_vec, message, self.get_latest_blockhash())
    }

    /// Assemble the given instructions into a transaction and sign it. All transactions constructed by this method are signed and payed for by the new_payer.
    fn tx_with_instructions_with_payer(
        &self,
        instructions: &[Instruction],
        signers: &[&Keypair],
        new_payer: Keypair,
    ) -> Transaction {
        let payer = new_payer;
        let mut signer_vec = vec![&payer];
        signer_vec.extend_from_slice(signers);

        let message = Message::new(instructions, Some(&payer.pubkey()));
        let num_sigs: usize = message.header.num_required_signatures.into();
        let required_sigs = message.account_keys[..num_sigs]
            .into_iter()
            .copied()
            .collect::<HashSet<_>>();
        let provided_sigs = signer_vec
            .iter()
            .map(|x| x.pubkey())
            .collect::<HashSet<_>>();

        for key in required_sigs.difference(&provided_sigs) {
            println!("missing signature from {}", key.to_string());
        }

        for key in provided_sigs.difference(&required_sigs) {
            println!("unnecessary signature from {}", key.to_string());
        }

        Transaction::new(&signer_vec, message, self.get_latest_blockhash())
    }

    /// Assemble the given instructions into a transaction and sign it. All transactions executed by this method are signed and payed for by the payer.
    fn execute_as_transaction(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> EncodedConfirmedTransactionWithStatusMeta {
        let tx = self.tx_with_instructions(instructions, signers);
        self.execute_transaction(tx)
    }

    /// Assemble the given instructions into a transaction and sign it. All transactions executed by this method are signed and payed for by the payer.
    fn execute_as_transaction_with_payer(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
        new_payer: Keypair,
    ) -> EncodedConfirmedTransactionWithStatusMeta {
        let tx = self.tx_with_instructions_with_payer(instructions, signers, new_payer);
        self.execute_transaction(tx)
    }

    /// Assemble the given instructions into a transaction and sign it. All transactions executed by this method are signed and payed for by the payer.
    /// Prints the transaction before sending it.
    fn execute_as_transaction_debug(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
    ) -> EncodedConfirmedTransactionWithStatusMeta {
        let tx = self.tx_with_instructions(instructions, signers);
        println!("{:#?}", &tx);
        self.execute_transaction(tx)
    }

    /// Assemble the given instructions into a transaction and sign it. All transactions executed by this method are signed and payed for by the new_payer.
    /// Prints the transaction before sending it.
    fn execute_as_transaction_with_payer_debug(
        &mut self,
        instructions: &[Instruction],
        signers: &[&Keypair],
        new_payer: Keypair,
    ) -> EncodedConfirmedTransactionWithStatusMeta {
        let tx = self.tx_with_instructions_with_payer(instructions, signers, new_payer);
        println!("{:#?}", &tx);
        self.execute_transaction(tx)
    }

    /// Executes a transaction constructing an empty account with the specified amount of space and lamports, owned by the provided program.
    fn create_account(&mut self, keypair: &Keypair, lamports: u64, space: usize, owner: Pubkey) {
        self.execute_transaction(system_transaction::create_account(
            &self.payer(),
            &keypair,
            self.get_latest_blockhash(),
            lamports,
            space as u64,
            &owner,
        ))
        .assert_success();
    }

    /// Executes a transaction constructing an empty rent-excempt account with the specified amount of space, owned by the provided program.
    fn create_account_rent_excempt(&mut self, keypair: &Keypair, space: usize, owner: Pubkey) {
        self.execute_transaction(system_transaction::create_account(
            &self.payer(),
            &keypair,
            self.get_latest_blockhash(),
            self.get_rent_excemption(space),
            space as u64,
            &owner,
        ))
        .assert_success();
    }

    /// Executes a transaction constructing a token mint. The account needs to be empty and belong to system for this to work.
    fn create_token_mint(
        &mut self,
        mint: &Keypair,
        authority: Pubkey,
        freeze_authority: Option<Pubkey>,
        decimals: u8,
    ) {
        self.execute_as_transaction(
            &[
                system_instruction::create_account(
                    &self.payer().pubkey(),
                    &mint.pubkey(),
                    self.get_rent_excemption(spl_token::state::Mint::LEN),
                    spl_token::state::Mint::LEN as u64,
                    &spl_token::ID,
                ),
                spl_token::instruction::initialize_mint(
                    &spl_token::ID,
                    &mint.pubkey(),
                    &authority,
                    freeze_authority.as_ref(),
                    decimals,
                )
                .unwrap(),
            ],
            &[mint],
        )
        .assert_success();
    }

    /// Executes a transaction that mints tokens from a mint to an account belonging to that mint.
    fn mint_tokens(&mut self, mint: Pubkey, authority: &Keypair, account: Pubkey, amount: u64) {
        self.execute_as_transaction(
            &[spl_token::instruction::mint_to(
                &spl_token::ID,
                &mint,
                &account,
                &authority.pubkey(),
                &[],
                amount,
            )
            .unwrap()],
            &[authority],
        )
        .assert_success();
    }

    /// Executes a transaction constructing a token account of the specified mint. The account needs to be empty and belong to system for this to work.
    /// Prefer to use [create_associated_token_account] if you don't need the provided account to contain the token account.
    fn create_token_account(&mut self, account: &Keypair, mint: Pubkey) {
        self.execute_as_transaction(
            &[
                system_instruction::create_account(
                    &self.payer().pubkey(),
                    &account.pubkey(),
                    self.get_rent_excemption(spl_token::state::Account::LEN),
                    spl_token::state::Account::LEN as u64,
                    &spl_token::ID,
                ),
                spl_token::instruction::initialize_account(
                    &spl_token::ID,
                    &account.pubkey(),
                    &mint,
                    &account.pubkey(),
                )
                .unwrap(),
            ],
            &[account],
        )
        .assert_success();
    }

    /// Executes a transaction constructing the associated token account of the specified mint belonging to the owner. This will fail if the account already exists.
    fn create_associated_token_account(&mut self, owner: &Keypair, mint: Pubkey) -> Pubkey {
        self.execute_as_transaction(
            &[
                spl_associated_token_account::instruction::create_associated_token_account(
                    &self.payer().pubkey(),
                    &owner.pubkey(),
                    &mint,
                    &spl_token::ID,
                ),
            ],
            &[],
        );
        get_associated_token_address(&owner.pubkey(), &mint)
    }

    /// Executes a transaction constructing the associated token account of the specified mint belonging to the owner.
    fn get_or_create_associated_token_account(&mut self, owner: &Keypair, mint: Pubkey) -> Pubkey {
        let acc = get_associated_token_address(&owner.pubkey(), &mint);
        if self.get_account(acc).is_none() {
            self.create_associated_token_account(owner, mint);
        }
        acc
    }

    /// Execute a transaction creating and filling a given account with the given data.
    /// The account is required to be empty and will be owned by spl_shared_memory afterwards.
    // Development note: Prefer this function due to efficiencies.
    fn create_account_with_data(&mut self, account: &Keypair, data: &[u8]) {
        let shared_memory_program_id: Pubkey = "shmem4EWT2sPdVGvTZCzXXRAURL9G5vpPxNwSeKhHUL"
            .parse()
            .unwrap();
        // Calculate the largest chunk size that can be written to the account in a single transaction.
        let chunk_size = PACKET_DATA_SIZE.saturating_sub(1).saturating_sub(
            bincode::serialized_size(&Transaction {
                signatures: vec![Signature::default()],
                message: Message::new(
                    &[Instruction::new_with_bincode(
                        shared_memory_program_id,
                        &[0u64],
                        vec![AccountMeta::new(account.pubkey(), false)],
                    )],
                    Some(&self.payer().pubkey()),
                ),
            })
            .unwrap() as usize,
        );

        self.execute_as_transaction(
            &[system_instruction::create_account(
                &self.payer().pubkey(),
                &account.pubkey(),
                self.get_rent_excemption(data.len()),
                data.len() as u64,
                &shared_memory_program_id,
            )],
            &[&self.payer(), account],
        )
        .assert_success();

        let mut offset = 0usize;
        for chunk in data.chunks(chunk_size) {
            println!("writing bytes {} to {}", offset, offset + chunk.len());
            let tx_data = [&(offset as u64).to_le_bytes(), chunk].concat();
            self.execute_as_transaction(
                &[Instruction::new_with_bytes(
                    shared_memory_program_id,
                    &tx_data,
                    vec![AccountMeta::new(account.pubkey(), false)],
                )],
                &[],
            )
            .assert_success();
            offset += chunk.len();
        }
    }

    /// Execute a transaction creating and filling a given account with the given data.
    /// Serializes the data using bincode.
    fn create_account_with_bincode<T: ?Sized + serde::Serialize>(
        &mut self,
        account: &Keypair,
        data: &T,
    ) {
        self.create_account_with_data(account, &bincode::serialize(&data).unwrap());
    }

    /// Execute a transaction creating and filling a given account with the given data.
    /// Serializes the data using borsh.
    fn create_account_with_borsh<T: BorshSerialize>(&mut self, account: &Keypair, data: &T) {
        self.create_account_with_data(account, &data.try_to_vec().unwrap());
    }

    /// Executes a transaction creating and filling the given account with the given data.
    /// The account is required to be empty and will be owned by bpf_loader afterwards.
    fn create_program_account_with_data(&mut self, account: &Keypair, data: &[u8]) {
        self.execute_transaction(system_transaction::create_account(
            &self.payer(),
            account,
            self.get_latest_blockhash(),
            self.get_rent_excemption(data.len()),
            data.len() as u64,
            &bpf_loader::id(),
        ))
        .assert_success();

        let mut offset = 0usize;
        for chunk in data.chunks(900) {
            println!("writing bytes {} to {}", offset, offset + chunk.len());
            self.execute_as_transaction(
                &[loader_instruction::write(
                    &account.pubkey(),
                    &bpf_loader::id(),
                    offset as u32,
                    chunk.to_vec(),
                )],
                &[account],
            )
            .assert_success();
            offset += chunk.len();
        }
    }

    /// Executes a transaction deploying a program from a file if it does not already exist.
    /// The keypair is derived from the file contents.
    fn deploy_program<P: AsRef<Path>>(&mut self, program_path: P) -> Pubkey {
        let data = std::fs::read(program_path).unwrap();
        let mut hash = Sha256::default();
        hash.update(&data);
        let mut rng = StdRng::from_seed(hash.finalize()[..].try_into().unwrap());
        let keypair = Keypair::generate(&mut rng);

        if self.get_account(keypair.pubkey()).is_none() {
            self.create_program_account_with_data(&keypair, &data);
            self.execute_as_transaction(
                &[loader_instruction::finalize(
                    &keypair.pubkey(),
                    &bpf_loader::id(),
                )],
                &[&keypair],
            )
            .assert_success();
        }

        keypair.pubkey()
    }

    /// Gets and unpacks an account. None if the account does not exist.
    fn get_unpacked_account<T: Pack>(&self, pubkey: Pubkey) -> Option<T> {
        let acc = self.get_account(pubkey)?;
        Some(T::unpack_unchecked(&acc.data).unwrap())
    }

    /// Gets and deserializes an account. None if the account does not exist.
    fn get_deserialized_account<T: BorshDeserialize>(&self, pubkey: Pubkey) -> Option<T> {
        let acc = self.get_account(pubkey)?;
        Some(T::try_from_slice(&acc.data).unwrap())
    }

    /// Gets and deserializes an account. None if the account does not exist.
    fn get_serde_deserialized_account<'a, T: DeserializeOwned>(&self, pubkey: Pubkey) -> Option<T> {
        let acc = self.get_account(pubkey)?;
        Some(bincode::deserialize(&acc.data).unwrap())
    }
}

/// An clean environment that executes transactions locally. Good for testing and debugging.
/// This environment has the most important SPL programs: spl-token, spl-associated-token-account and spl-memo v1 and v3.
pub struct LocalEnvironment {
    bank: Bank,
    faucet: Keypair,
}

impl LocalEnvironment {
    /// Constructs a builder for a local environment
    pub fn builder() -> LocalEnvironmentBuilder {
        LocalEnvironmentBuilder::new()
    }

    /// Constructs a clean local environment.
    pub fn new() -> LocalEnvironment {
        Self::builder().build()
    }

    pub fn bank(&mut self) -> &mut Bank {
        &mut self.bank
    }

    /// Advance the bank to the next blockhash.
    pub fn advance_blockhash(self) -> Self {
        let new_slot = self.bank.slot().saturating_add(1);

        while !self.bank.is_complete() {
            self.bank.register_tick(&Hash::new_unique());
        }

        LocalEnvironment {
            bank: Bank::new_from_parent(Arc::new(self.bank), &self.faucet.pubkey(), new_slot),
            faucet: self.faucet,
        }
    }

    pub fn execute_transactions<T>(
        &mut self,
        txs: Vec<T>,
    ) -> Vec<EncodedConfirmedTransactionWithStatusMeta>
    where
        VersionedTransaction: From<T>,
    {
        let txs = txs
            .into_iter()
            .map(|tx| {
                let tx = VersionedTransaction::from(tx);
                let len = bincode::serialize(&tx).unwrap().len();
                if len > packet::PACKET_DATA_SIZE {
                    panic!(
                        "tx {:?} of size {} is {} too large",
                        tx,
                        len,
                        len - packet::PACKET_DATA_SIZE
                    )
                }
                tx
            })
            .collect::<Vec<_>>();
        let batch = self.bank.prepare_entry_batch(txs.clone()).unwrap();
        let txs_sanitized = batch.sanitized_transactions();

        let mut mint_decimals = HashMap::new();
        let tx_pre_token_balances = solana_ledger::token_balances::collect_token_balances(
            &self.bank,
            &batch,
            &mut mint_decimals,
        );
        let slot = self.bank.slot();
        let mut timings = Default::default();
        let (
            TransactionResults {
                execution_results, ..
            },
            TransactionBalancesSet {
                pre_balances,
                post_balances,
                ..
            },
        ) = self.bank.load_execute_and_commit_transactions(
            &batch,
            usize::MAX,
            true,
            true,
            true,
            true,
            &mut timings,
            None,
        );

        let tx_post_token_balances = solana_ledger::token_balances::collect_token_balances(
            &self.bank,
            &batch,
            &mut mint_decimals,
        );

        let mut encoded_confirmed_txs = vec![];

        for (
            tx,
            execution_result,
            pre_balances,
            post_balances,
            pre_token_balances,
            post_token_balances,
            tx_sanitized,
        ) in izip!(
            txs.iter(),
            execution_results.into_iter(),
            pre_balances.into_iter(),
            post_balances.into_iter(),
            tx_pre_token_balances.into_iter(),
            tx_post_token_balances.into_iter(),
            txs_sanitized.into_iter(),
        ) {
            let fee = self
                .bank
                .get_fee_for_message(tx_sanitized.message())
                .expect("Fee calculation must succeed");

            let status;
            let inner_instructions;
            let log_messages;
            let return_data;
            let compute_units_consumed;

            match execution_result {
                TransactionExecutionResult::Executed { details, .. } => {
                    status = details.status;
                    inner_instructions = details.inner_instructions;
                    log_messages = details.log_messages;
                    return_data = details.return_data;
                    compute_units_consumed = Some(details.executed_units);
                }
                TransactionExecutionResult::NotExecuted(err) => {
                    status = Err(err);
                    inner_instructions = None;
                    log_messages = None;
                    return_data = None;
                    compute_units_consumed = None;
                }
            }

            let inner_instructions = inner_instructions.map(|inner_instructions| {
                inner_instructions
                    .into_iter()
                    .enumerate()
                    .map(|(index, instructions)| {
                        let inner_ixs_mapped = instructions
                            .into_iter()
                            .map(|x| solana_transaction_status::InnerInstruction {
                                instruction: x.instruction,
                                stack_height: Some(x.stack_height as u32),
                            })
                            .collect();
                        InnerInstructions {
                            index: index as u8,
                            instructions: inner_ixs_mapped,
                        }
                    })
                    .filter(|i| !i.instructions.is_empty())
                    .collect()
            });
            let tx_status_meta = TransactionStatusMeta {
                status,
                fee,
                pre_balances,
                post_balances,
                pre_token_balances: Some(pre_token_balances),
                post_token_balances: Some(post_token_balances),
                inner_instructions,
                log_messages,
                rewards: None,
                loaded_addresses: tx_sanitized.get_loaded_addresses(),
                return_data,
                compute_units_consumed,
            };
            encoded_confirmed_txs.push(
                ConfirmedTransactionWithStatusMeta {
                    slot,
                    tx_with_meta: TransactionWithStatusMeta::Complete(
                        VersionedTransactionWithStatusMeta {
                            transaction: tx.clone(),
                            meta: tx_status_meta,
                        },
                    ),
                    block_time: Some(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs()
                            .try_into()
                            .unwrap(),
                    ),
                }
                .encode(UiTransactionEncoding::Binary, Some(0))
                .expect("Failed to encode transaction"),
            );
        }
        encoded_confirmed_txs
    }
}

impl Environment for LocalEnvironment {
    fn payer(&self) -> Keypair {
        clone_keypair(&self.faucet)
    }

    fn execute_transaction<T>(&mut self, tx: T) -> EncodedConfirmedTransactionWithStatusMeta
    where
        VersionedTransaction: From<T>,
    {
        let tx: VersionedTransaction = tx.into();
        self.execute_transactions::<VersionedTransaction>(vec![tx])
            .into_iter()
            .next()
            .unwrap()
    }

    fn get_latest_blockhash(&self) -> Hash {
        self.bank.last_blockhash()
    }

    fn get_rent_excemption(&self, data: usize) -> u64 {
        self.bank.get_minimum_balance_for_rent_exemption(data)
    }

    fn get_account(&self, pubkey: Pubkey) -> Option<Account> {
        self.bank.get_account(&pubkey).map(|acc| acc.into())
    }
}

pub struct LocalEnvironmentBuilder {
    config: GenesisConfig,
    faucet: Keypair,
}

impl LocalEnvironmentBuilder {
    fn new() -> Self {
        let faucet = random_keypair();
        let mut config = GenesisConfig::new(
            &[(
                faucet.pubkey(),
                AccountSharedData::new(1u64 << 48, 0, &system_program::id()),
            )],
            &[],
        );
        genesis_utils::activate_all_features(&mut config);

        // Deactiveate delay_visibility_of_program_deployment to allow to programs to run on the same slot they are deployed on
        config
            .accounts
            .remove(&feature_set::delay_visibility_of_program_deployment::id());

        let mut builder = LocalEnvironmentBuilder { faucet, config };
        builder.add_account_with_data(
            spl_associated_token_account::ID,
            bpf_loader::ID,
            programs::SPL_ASSOCIATED_TOKEN,
            true,
        );
        builder.add_account_with_data(
            "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo"
                .parse()
                .unwrap(),
            bpf_loader::ID,
            programs::SPL_MEMO1,
            true,
        );
        builder.add_account_with_data(spl_memo::ID, bpf_loader::ID, programs::SPL_MEMO3, true);
        builder.add_account_with_data(spl_token::ID, bpf_loader::ID, programs::SPL_TOKEN, true);
        builder.add_account_with_data(
            spl_token_2022::ID,
            bpf_loader::ID,
            programs::SPL_TOKEN_2022,
            true,
        );
        builder.add_account_with_data(
            "shmem4EWT2sPdVGvTZCzXXRAURL9G5vpPxNwSeKhHUL"
                .parse()
                .unwrap(),
            bpf_loader::ID,
            programs::SPL_SHARED_MEMORY,
            true,
        );
        builder.add_account_with_lamports(rent::ID, sysvar::ID, 1);
        builder
    }

    /// Sets the creation time of the network
    pub fn set_creation_time(&mut self, unix_timestamp: UnixTimestamp) -> &mut Self {
        self.config.creation_time = unix_timestamp as UnixTimestamp;
        self
    }

    /// Adds the account into the environment.
    pub fn add_account(&mut self, pubkey: Pubkey, account: Account) -> &mut Self {
        self.config.add_account(pubkey, account.into());
        self
    }

    /// Reads the program from the path and add it at the address into the environment.
    pub fn add_program<P: AsRef<Path>>(&mut self, pubkey: Pubkey, path: P) -> &mut Self {
        self.add_account_with_data(pubkey, bpf_loader::ID, &std::fs::read(path).unwrap(), true);
        self
    }

    // Adds a rent-excempt account into the environment.
    pub fn add_account_with_data(
        &mut self,
        pubkey: Pubkey,
        owner: Pubkey,
        data: &[u8],
        executable: bool,
    ) -> &mut Self {
        self.add_account(
            pubkey,
            Account {
                lamports: self.config.rent.minimum_balance(data.len()),
                data: data.to_vec(),
                executable,
                owner,
                rent_epoch: 0,
            },
        )
    }

    // Adds an account with the given balance into the environment.
    pub fn add_account_with_lamports(
        &mut self,
        pubkey: Pubkey,
        owner: Pubkey,
        lamports: u64,
    ) -> &mut Self {
        self.add_account(
            pubkey,
            Account {
                lamports,
                data: vec![],
                executable: false,
                owner,
                rent_epoch: 0,
            },
        )
    }

    // Adds a rent-excempt account into the environment.
    pub fn add_account_with_packable<P: Pack>(
        &mut self,
        pubkey: Pubkey,
        owner: Pubkey,
        data: P,
    ) -> &mut Self {
        let data = {
            let mut buf = vec![0u8; P::LEN];
            data.pack_into_slice(&mut buf[..]);
            buf
        };
        self.add_account_with_data(pubkey, owner, &data, false)
    }

    // Add a token-mint into the environment.
    pub fn add_token_mint(
        &mut self,
        pubkey: Pubkey,
        mint_authority: Option<Pubkey>,
        supply: u64,
        decimals: u8,
        freeze_authority: Option<Pubkey>,
    ) -> &mut Self {
        self.add_account_with_packable(
            pubkey,
            spl_token::ID,
            spl_token::state::Mint {
                mint_authority: COption::from(mint_authority.map(|c| c.clone())),
                supply,
                decimals,
                is_initialized: true,
                freeze_authority: COption::from(freeze_authority.map(|c| c.clone())),
            },
        )
    }

    // Add a token-account into the environment.
    pub fn add_account_with_tokens(
        &mut self,
        pubkey: Pubkey,
        mint: Pubkey,
        owner: Pubkey,
        amount: u64,
    ) -> &mut Self {
        self.add_account_with_packable(
            pubkey,
            spl_token::ID,
            spl_token::state::Account {
                mint,
                owner,
                amount,
                delegate: COption::None,
                state: spl_token::state::AccountState::Initialized,
                is_native: COption::None,
                delegated_amount: 0,
                close_authority: COption::None,
            },
        )
    }

    // Add the associated token-account into the environment.
    pub fn add_associated_account_with_tokens(
        &mut self,
        owner: Pubkey,
        mint: Pubkey,
        amount: u64,
    ) -> &mut Self {
        self.add_account_with_packable(
            get_associated_token_address(&owner, &mint),
            spl_token::ID,
            spl_token::state::Account {
                mint,
                owner,
                amount,
                delegate: COption::None,
                state: spl_token::state::AccountState::Initialized,
                is_native: COption::None,
                delegated_amount: 0,
                close_authority: COption::None,
            },
        )
    }

    /// Clone an account from a cluster using the given rpc client. Use [clone_upgradable_program_from_cluster] if you want to clone a upgradable program, as this requires multiple accounts.
    pub fn clone_account_from_cluster(&mut self, pubkey: Pubkey, client: &RpcClient) -> &mut Self {
        println!("Loading account {} from cluster", pubkey);
        let account = client
            .get_account(&pubkey)
            .expect("couldn't retrieve account");
        self.add_account(
            pubkey,
            Account {
                lamports: account.lamports,
                data: account.data,
                executable: account.executable,
                owner: account.owner,
                rent_epoch: 0,
            },
        )
    }

    /// Clone multiple accounts from a cluster using the given rpc client.
    pub fn clone_accounts_from_cluster(
        &mut self,
        pubkeys: &[Pubkey],
        client: &RpcClient,
    ) -> &mut Self {
        for &pubkey in pubkeys {
            self.clone_account_from_cluster(pubkey, client);
        }
        self
    }

    /// Clones all accounts required to execute the given executable program from the cluster, using the given rpc client.
    pub fn clone_upgradable_program_from_cluster(
        &mut self,
        client: &RpcClient,
        pubkey: Pubkey,
    ) -> &mut Self {
        println!("Loading upgradable program {} from cluster", pubkey);
        let account = client
            .get_account(&pubkey)
            .expect("couldn't retrieve account");
        let upgradable: UpgradeableLoaderState = account.deserialize_data().unwrap();
        if let UpgradeableLoaderState::Program {
            programdata_address,
        } = upgradable
        {
            self.add_account(pubkey, account);
            self.clone_account_from_cluster(programdata_address, client);
        } else {
            panic!("Account is not an upgradable program")
        }
        self
    }

    /// Finalizes the environment.
    pub fn build(&mut self) -> LocalEnvironment {
        let tmpdir = Path::new("/tmp/");
        let exit = Arc::new(AtomicBool::new(false));
        let bank = Bank::new_with_paths(
            &self.config,
            Arc::new(RuntimeConfig::default()),
            vec![tmpdir.to_path_buf()],
            None,
            None,
            AccountSecondaryIndexes {
                keys: None,
                indexes: HashSet::new(),
            },
            AccountShrinkThreshold::default(),
            false,
            None,
            None,
            exit,
        );

        let env = LocalEnvironment {
            bank,
            faucet: clone_keypair(&self.faucet),
        };
        env.advance_blockhash()
    }
}

/// A remote environment on a cluster. Interacts with the cluster using RPC.
pub struct RemoteEnvironment {
    client: RpcClient,
    payer: Keypair,
}

impl RemoteEnvironment {
    /// Contruct a new remote environment. The payer keypair is expected to have enough funds to fund all transactions.
    pub fn new(client: RpcClient, payer: Keypair) -> Self {
        RemoteEnvironment { client, payer }
    }

    /// Construct a new remote environment, airdropping lamports from the given airdrop endpoint up to the given account. Use this on devnet and testnet.
    pub fn new_with_airdrop(client: RpcClient, payer: Keypair, lamports: u64) -> Self {
        let env = RemoteEnvironment { client, payer };
        env.airdrop(env.payer().pubkey(), lamports);
        env
    }

    /// Airdrop lamports up to the given balance to the account.
    pub fn airdrop(&self, account: Pubkey, lamports: u64) {
        if self.client.get_balance(&account).expect("get balance") < lamports {
            println!("Requesting airdrop...");
            let blockhash = self.client.get_latest_blockhash().unwrap();
            let sig = self
                .client
                .request_airdrop_with_blockhash(&account, lamports, &blockhash)
                .unwrap();
            self.client
                .confirm_transaction_with_spinner(&sig, &blockhash, CommitmentConfig::confirmed())
                .unwrap();
        }
    }
}

impl Environment for RemoteEnvironment {
    fn payer(&self) -> Keypair {
        clone_keypair(&self.payer)
    }

    fn execute_transaction<T>(&mut self, tx: T) -> EncodedConfirmedTransactionWithStatusMeta
    where
        VersionedTransaction: From<T>,
    {
        let tx = VersionedTransaction::from(tx);
        let sig = match self.client.send_and_confirm_transaction(&tx) {
            Err(e) => panic!("{:#?}", e),
            Ok(sig) => sig,
        };
        self.client
            .get_transaction_with_config(
                &sig,
                RpcTransactionConfig {
                    encoding: Some(UiTransactionEncoding::Binary),
                    commitment: Some(CommitmentConfig::confirmed()),
                    max_supported_transaction_version: Some(0),
                    ..RpcTransactionConfig::default()
                },
            )
            .unwrap()
    }

    fn get_latest_blockhash(&self) -> Hash {
        self.client.get_latest_blockhash().unwrap()
    }

    fn get_rent_excemption(&self, data: usize) -> u64 {
        self.client
            .get_minimum_balance_for_rent_exemption(data)
            .unwrap()
    }

    fn get_account(&self, pubkey: Pubkey) -> Option<Account> {
        self.client
            .get_account_with_commitment(&pubkey, self.client.commitment())
            .unwrap()
            .value
    }
}

/// Utility trait for printing transaction results.
pub trait PrintableTransaction {
    /// Pretty print the transaction results, tagged with the given name for distinguishability.
    fn print_named(&self, name: &str);

    /// Pretty print the transaction results.
    fn print(&self) {
        self.print_named("");
    }

    /// Panic and print the transaction if it did not execute successfully
    fn assert_success(&self);
}

impl PrintableTransaction for ConfirmedTransactionWithStatusMeta {
    fn print_named(&self, name: &str) {
        let tx = self.tx_with_meta.get_transaction();
        let encoded = self
            .clone()
            .encode(UiTransactionEncoding::JsonParsed, None)
            .expect("Failed to encode");
        println!("EXECUTE {} (slot {})", name, encoded.slot);
        println_transaction(&tx, encoded.transaction.meta.as_ref(), "  ", None, None);
    }

    fn assert_success(&self) {
        match &self.tx_with_meta.get_status_meta() {
            Some(meta) if meta.status.is_err() => {
                self.print();
                panic!("tx failed!")
            }
            _ => (),
        }
    }
}

impl PrintableTransaction for EncodedConfirmedTransactionWithStatusMeta {
    fn print_named(&self, name: &str) {
        let tx = self.transaction.transaction.decode().unwrap();
        println!("EXECUTE {} (slot {})", name, self.slot);
        println_transaction(&tx, self.transaction.meta.as_ref(), "  ", None, None);
    }

    fn assert_success(&self) {
        match &self.transaction.meta {
            Some(meta) if meta.err.is_some() => {
                self.print();
                panic!("tx failed!")
            }
            _ => (),
        }
    }
}

pub enum LogLevel {
    TRACE,
    DEBUG,
    INFO,
    WARN,
    ERROR,
}

/// Setup solana logging. This is heavily recommended if you're using a local environment.
pub fn setup_logging(level: LogLevel) {
    match level {
        LogLevel::TRACE => solana_logger::setup_with_default(
            "trace,solana_runtime::message_processor=trace,solana_metrics::metrics=error",
        ),
        LogLevel::DEBUG => solana_logger::setup_with_default(
            "debug,solana_runtime::message_processor=debug,solana_metrics::metrics=error",
        ),
        LogLevel::INFO => solana_logger::setup_with_default(
            "info,solana_runtime::message_processor=info,solana_metrics::metrics=error",
        ),
        LogLevel::WARN => solana_logger::setup_with_default(
            "warn,solana_runtime::message_processor=warn,solana_metrics::metrics=error",
        ),
        LogLevel::ERROR => solana_logger::setup_with_default(
            "error,solana_runtime::message_processor=error,solana_metrics::metrics=error",
        ),
    }
}

/// Clone the given keypair.
pub fn clone_keypair(keypair: &Keypair) -> Keypair {
    Keypair::from_bytes(&keypair.to_bytes()).unwrap()
}

/// Generate a random keypair.
pub fn random_keypair() -> Keypair {
    Keypair::generate(&mut OsRng::default())
}

/// Return a recognisable Keypair. The public key will start with `Kxxx`, where xxx are the three digits of the number.
/// `o` is used instead of `0`, as `0` is not part of the base58 charset.
pub fn keypair(n: u8) -> Keypair {
    Keypair::from_bytes(&keys::KEYPAIRS[n as usize]).unwrap()
}

/// Constructs a devnet client using `CommitmentConfig::confirmed()`.
pub fn devnet_client() -> RpcClient {
    RpcClient::new_with_commitment(
        "https://api.devnet.solana.com/".to_string(),
        CommitmentConfig::confirmed(),
    )
}

/// Constructs a testnet client using `CommitmentConfig::confirmed()`.
pub fn testnet_client() -> RpcClient {
    RpcClient::new_with_commitment(
        "https://api.testnet.solana.com/".to_string(),
        CommitmentConfig::confirmed(),
    )
}

/// Constructs a client connecting to localhost:8899 using `CommitmentConfig::confirmed()`.
pub fn localhost_client() -> RpcClient {
    RpcClient::new_with_commitment(
        "http://localhost:8899/".to_string(),
        CommitmentConfig::confirmed(),
    )
}
