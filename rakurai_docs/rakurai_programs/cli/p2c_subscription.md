# P2C Subscription CLI (`rakurai-p2c`)

**This is how you keep your post-pack stream paid for.** Use it to see what a validator's subscription costs, check what you owe for past epochs, and top up before the grace window runs out and delivery stops. Updates missed while suspended are not replayed.

It touches the **PSA** only. Tips and backrun settlement are separate paths with separate tools.

**Audience:** P2C User/Consumer (searchers / traders / services that consume Rakurai post-pack confirmations).

**Program:** Reward Distribution **PSA** (`P2CSubscriptionAccount`).

Install: [CLI overview](./README.md#2-installation). How billing works on-chain: [PSA](../programs/reward_distribution/README.md#5-psa--prepaid-fee-to-use-post-pack).

---

## 1. What this CLI is about

Post-pack confirmations need a **prepaid subscription escrow** (PSA) per service name + validator vote. You keep that escrow funded; each epoch a fee based on Updates/txn volume (**$50 / 1M txn updates**) is deducted from it. If the escrow runs dry and a deficit builds up, P2C access can move to grace and then stop until the shortfall is cleared.

This CLI is how **you** manage funding for that escrow:

- **Read** balance, total owed, status, and deficit (`get-account`, `get-all-accounts`)
- **Fund** prepaid SOL (`fund`, `fund-all`, or a plain `solana transfer` to the PDA)

This is **not** MevShare revenue sharing. Sharing post-pack backrun profit uses an **MCA** and [`rakurai-revshare`](./partner_reward_settlement.md). Post-pack product overview: [official guide](https://docs.rakurai.io/docs/services/rakurai_jito_private/rakurai_docs/transaction_inclusion/post_pack/).

PSA accounts are created by Rakurai / ops before you fund them.

---

## 2. What you do each epoch

1. Keep the prepaid account funded **before** the epoch ends (`fund` / `fund-all`, or `solana transfer` to the PDA).
2. After the epoch, run `get-account` and check:
   - balance after rent-exempt
   - total owed (excludes the current in-progress epoch)
   - status (`Active` / `InGrace` / `Suspended`)
   - deficit (shown only when greater than zero)
   - underfunded alert (only when balance < owed for past epochs)
3. If balance is low, `fund` or `fund-all`.

`get-all-accounts` lists every subscription for the same `--name`. `--detail` adds manager/auth and per-epoch owed rows.

### 2.1. Status meanings

| Status | Meaning |
|--------|---------|
| `Active` | Fees current; P2C eligible |
| `InGrace` | Recent underfunded periods within the grace window (default **2** epochs) |
| `Suspended` | Unpaid streak exceeded grace — **P2C access is stopped** until the shortfall is cleared |

> [!CAUTION]
> **Suspended stops the stream**
>
> Once the status reaches `Suspended`, the validator stops sending you post-pack updates. Funding the shortfall clears the deficit and restores delivery, but any updates missed while suspended are gone. Watch the `deficit` field and top up before the streak reaches `grace_epochs`.

> [!NOTE]
> **Owed always excludes the current epoch**
>
> `total owed` and the `fund-all` shortfall calculation cover **past epochs only**. The in-progress cluster epoch has not been priced yet, so a balance that looks sufficient today can still be short once the epoch closes.

---

## 3. Commands

```sh
rakurai-p2c \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <KEYPAIR> \
  <COMMAND>
```

Global flags: `-k` / `--keypair`, `-u` / `--url`, `-p` / `--program-id`.

| Command | Purpose |
|---------|---------|
| `get-account` | Show one escrow (balance after rent-exempt, total owed, status, deficit if any) |
| `get-all-accounts` | List escrows for a service `--name` |
| `fund` | Top up prepaid SOL |
| `fund-all` | Fund shortfalls for every underfunded PSA under `--name` (excludes current epoch) |

### 3.1. `get-account`

One escrow for one validator. Summary: pubkey, name, vote, status, balance after rent-exempt, **total owed** (past epochs only — current cluster epoch excluded), and **deficit only if greater than zero**. An underfunded alert appears only when balance is below owed. `--detail` adds manager/auth, commission, and per-epoch rows (`due` / `deducted` / `owed` / `claimed`).

```sh
rakurai-p2c \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-account \
  --name <SERVICE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY>
```

### 3.2. `get-all-accounts`

```sh
rakurai-p2c \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-all-accounts \
  --name <SERVICE_NAME>
```

### 3.3. `fund`

```sh
rakurai-p2c \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <FUNDER_KEYPAIR> \
  fund \
  --name <SERVICE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY> \
  --amount <LAMPORTS>
```

Anyone may also fund the PDA with a plain `solana transfer` into the escrow address.

### 3.4. `fund-all`

Funds the shortfall (`owed − balance`) for every underfunded PSA with the same `--name`. Owed excludes the current cluster epoch. Skips accounts that already cover past-epoch due.

```sh
rakurai-p2c \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <FUNDER_KEYPAIR> \
  fund-all \
  --name <SERVICE_NAME>
```

`--batch-size` (default 10) controls fund instructions per transaction. Pass `--dry-run` to preview.

---

## Appendix: Program IDs

| Cluster | Reward Distribution program |
| ------- | --------------------------- |
| Mainnet | `RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB` |
| Testnet | `A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB` |
