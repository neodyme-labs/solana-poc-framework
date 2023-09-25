import os
from git import Repo
import toml
BRANCHES = [
    '1.11',
    '1.13',
    '1.14',
    '1.16',
]

IMPORTANT_MODULES = [
    "solana-sdk",
    "solana-program",
    "solana-logger",
    "solana-runtime",
    "solana-transaction-status",
    "solana-cli-output",
    "solana-bpf-loader-program",
    "solana-compute-budget-program",
    "solana-vote-program",
    "solana-stake-program",
    "solana-config-program",
    "solana-client",
    "solana-faucet",
    "solana-program-runtime",
    "solana-ledger",
]


r = Repo(os.path.dirname(os.path.realpath(__file__)))

assert r.heads.main == r.active_branch

for branch in BRANCHES:
    if branch not in r.heads:
        print(f'Creating branch {branch}')
        r.create_head(branch, r.heads.main)
        r.heads[branch].checkout()
        with open('Cargo.toml') as f:
            cargo = toml.load(f)
        for module in IMPORTANT_MODULES:
            cargo['dependencies'][module] = f'~{branch}'
        with open('Cargo.toml', 'w') as f:
            toml.dump(cargo, f)

        os.system("cargo generate-lockfile")

        r.index.add(['Cargo.toml', 'Cargo.lock'])
        r.index.commit(f'Create {branch} branch')
        #r.remotes.origin.push(branch)


    else:
        print("Updating branch %s" % branch)
        r.heads[branch].checkout()

        base = r.git.merge(r.heads.main)

r.heads.main.checkout()