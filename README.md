# Solana PoC Framework

[![](https://img.shields.io/crates/v/poc-framework)](https://crates.io/crates/poc-framework) [![](https://docs.rs/poc-framework/badge.svg)](https://docs.rs/poc-framework/)

**DISCLAIMER: any illegal usage of this framework is heavily discouraged. Most projects on Solana offer a more than generous bug bounty. Also you don't want your kneecaps broken.**

## Usage
To get started, just add the following line to the `dependencies` section in your `Cargo.toml`:
```toml
[dependencies]
poc-framework = "0.1.0"
```
This crate already re-exports every Solana dependency you should need.

## What this framework is for
This framework was made for security researchers, to facilitate a fast and convenient development of Proof-of-Concepts for bugs in Solana smart contracts or even Solana core. The generic `Environment` interface allows for exploits to be developed locally, and then tested on Testnet or Devnet.

## Feature overview

### Utility
This framework offers many utility functions that proved very useful time and time again for the PoC's we developed for the smart contrats we audited at Neodyme.

The first thing you want to do in any PoC is setup logging. This is especially useful if you use a local environment, as it is the only way to figure out why a transaction could not be executed (if for example signers are missing):
```rust
setup_logging(LogLevel::DEBUG);
```

Afterwards you want to define what keys you will use. Keys should easily be identifiable when printing a transaction. This purpoise gets fulfilled by the `keypair(n: u8)` function. The framework contains 256 pre-ground keys that start with `Kxxx`, where `xxx` is the 3-digit representation of the argument `n`. Note that the base58 charset does not contain `0`, which is why we used `o` instead:
```rust
let authority = keypair(0);   // KoooVyhdpoRPA6gpn7xr3cmjqAvtpHcjcBX6JBKu1nf
let target    = keypair(1);   // Koo1BQTQYawwKVBg71J2sru7W51EJgfbyyHsTFCssRW
let mint      = keypair(2);   // Koo2SZ393psmp7ags3hMz59ciV3XWLj1GkPousNgTH1
let victim    = keypair(137); // K137jwH7CncXBTadHbLDsHNWUhuLDN4ddegJL2hmn6u
```
There is also a `random_keypair` function if you don't care about recognising a keypair.

Also very valuable for debugging purpoises is the ability to print the result of a transaction in a neat way. For this the framework provides the trait `PrintableTransaction`, which it implements both for `ConfirmedTransaction` as well as `EncodedConfirmedTransaction`. This trait provides the function `print`, which can conviniently be chained to the end of any `env.execute_transaction` call:
```rust
env.execute_as_transaction(&[...], &[...]).print();
```



### Environment
At the core if this framework is the `Environment` trait. This encapsulates the ability to execute transactions on some chain state, as well as the utility of having a `payer` that pays for all fee and rent expenditures.

There are currently two different implementations: the `RemoteEnvironment` which executes all transactions on a cluster, and the `LocalEnvironment`, which executes all transactions locally on an arbitrary chain state.

The `Environment` trait also provides many useful shortcuts for sending transactions, like interacting with `spl-token` accounts or even creating accounts with arbitrary content (but obviously with a fixed owner).

#### RemoteEnvironment
To construct a remote environment, you require an `RpcClient`. These can be conveniently construced using `devnet_client()`/`testnet_client()`/`localhost_client()`. We do not condone using this framework on mainnet. Airdrops are also implemented, with the `new_with_airdrop` and `airdrop` functions.
```rust
let payer = read_keypair_file("big-fat-wallet.json").unwrap();
let client = devnet_client();
let mut env = RemoteEnvironment::new(client, payer);
```

#### LocalEnvironment
Constructing a local environment usually takes some effort, as one has to first clone the relevant chain state. The framework offers many different ways of doing this. From deploying a contract from a file to inserting an arbitrary account up to cloning accounts and even whole upgradable programs from a cluster:
```rust
let mut env = LocalEnvironment::builder()
    .add_account_with_lamports(authority, system_program::ID, sol_to_lamports(10.0))
    .add_token_mint(mint, Some(authority), 0, 1, None)
    .add_associated_token_account(authority, mint, 1337)
    .clone_upgradable_program_from_cluster(client, my_program::ID)
    .build();
```
Note however that it is possible to craft state that is not legal on the chain using this builder (for example accounts that belong to a program that contain state that the program itself would never write to it), leading to exploits that are only reproducible locally. Try to use transactions on the environment for as many things as possible to prevent these pitfalls.