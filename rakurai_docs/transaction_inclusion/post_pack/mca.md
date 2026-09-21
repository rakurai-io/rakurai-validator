# MCA — share backrun profit

The **MevShare Collection Account (MCA)** is how backrun partners pay the validator their agreed cut of backrun profit.

**PSA is included in the MCA.** You do **not** fund a separate [PSA](./psa.md) on this path — stream access (leader-time **and** TPU) is covered by settling MevShare through the MCA.

Settlement is self-reported because backrun profit lands in **your own wallet**. After each epoch: **report** what you owe, then **send** it. Miss the grace window and you lose post-pack access.

**Audience:** Searchers and TIN partners who backrun from P2C and share MevShare (not PSA-only resellers).

**Related:** [PSA](./psa.md) (prepaid path if you are *not* on MCA) · [Using P2C](./using_p2c.md) · [Revenue settlement](../revenue_settlement.md) · [`rakurai-revshare` CLI](../../rakurai_programs/cli/partner_reward_settlement.md)

> [!WARNING]
> **You must hold the MCA record authority**
>
> Reporting is signed by the MCA `record_authority` keypair. Without that key you cannot record what you owe, and therefore cannot settle — which eventually stops your post-pack access. Confirm which key it is with `rakurai-revshare get-account --detail` (`Record auth`) as soon as the account is created.

---

## 1. Who this is for

Searchers and TIN partners that:

- Capture MEV (backruns or arbitrage) from the post-pack stream
- **Share a percentage** of that profit with the validator through an MCA
- Use MCA for access (**PSA is included** — do not prepay a separate PSA for the same service / validator)

There is one MCA per **service** per **validator**, matching the PSA layout. The vault itself is `RevenueShareAccountV1` with `share_kind = MevShare`, derived from `[REVENUE_SHARE_V1, MEV_SHARE, name, vote]`.

> [!NOTE]
> **MCA and TCA are the same struct, different vaults**
>
> A custom **tip** account settles into a TCA (`--revenue-kind Tip`) and a post-pack **backrun** share settles into an MCA (`--revenue-kind Mev-share`). They use the same on-chain struct and the same CLI, but they are separate accounts and separate balances. Passing the wrong `--revenue-kind` targets the wrong vault.

---

## 2. Register

1. Onboard as for the PSA: share your endpoint and a wallet pubkey you control with the Rakurai team — [Setup guide](../setup_guide.md#4-contact-rakurai-and-share-your-wallet-pubkey).
2. Rakurai creates the **MCA** and gives you a **revenue name** plus the record key. The revenue name is a PDA seed — use the exact value Rakurai assigned, including case.
3. Confirm the account exists and the record authority matches your key:

```bash
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-account --detail \
  --revenue-kind Mev-share \
  --revenue-name <REVENUE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY>
```

4. Start consuming post-pack and settle after each epoch.

Partner vaults are created by Rakurai or ops. Creating one is not a partner CLI path.

---

## 3. During the epoch

**Nothing is taken automatically.** Profit stays in your accounts for the whole epoch and no ledger entry is written on your behalf.

This is the opposite of a custom tip account, where the validator records what is owed on every leader turn and you only transfer afterwards. For MevShare, **you** do both the recording and the transfer. See [TCA](../../rakurai_programs/programs/reward_distribution/README.md#4-tca--tips-for-landing-transactions) and [Tips — leader-turn stage](../tips.md#61-leader-turn-stage-every-leader-turn) for the contrast.

---

## 4. After the epoch

### 4.1. Report what you owe

`record-revenue` writes the amount into the epoch ledger. It moves **no SOL** and always targets the current cluster epoch.

```bash
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <RECORD_AUTHORITY_KEYPAIR> \
  record-revenue \
  --revenue-kind Mev-share \
  --revenue-name <REVENUE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY> \
  --amount <LAMPORTS>
```

> [!WARNING]
> **Record once per validator per epoch**
>
> `record-revenue` **adds to** the existing epoch entry rather than replacing it. Running it twice for the same epoch and vote books twice the amount owed, and you will then be expected to transfer that inflated figure. Check the current entry with `get-pending-record` before recording.

### 4.2. Send the SOL

`transfer` settles one epoch on one vault; `transfer-all` settles every pending epoch across matching vaults. Both accept `--dry-run`.

```bash
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <FUNDER_KEYPAIR> \
  transfer-all \
  --revenue-kind Mev-share \
  --revenue-name <REVENUE_NAME> \
  --dry-run
```

What is still outstanding is `pending = amount - transferred_amount`, visible per epoch with `get-pending-record` or `get-all-pending-records`.

### 4.3. What Rakurai does next

Rakurai's commission is deducted from what you sent, and the remainder is credited to the **validator identity**. With `block_reward_conversion_enabled` on (the default), that remainder is then converted into a high-priority block reward — the same treatment TCA and PSA revenue gets. Indexers should not count both — see the [indexing note](../README.md#6-do-not-double-count-tips-and-converted-block-rewards).

> [!CAUTION]
> **Settle within 2 epochs**
>
> If you do not report **and** send within roughly two epochs, post-pack access and MCA prioritization stop after that grace period.

---

## 5. CLI

Full command reference: **[Tip and MevShare Revenue Settlement CLI (`rakurai-revshare`)](../../rakurai_programs/cli/partner_reward_settlement.md)**. On-chain struct: [Reward Distribution — RevenueShareAccountV1](../../rakurai_programs/programs/reward_distribution/README.md#71-tca--mca--revenueshareaccountv1).

---

## Related

- [PSA](./psa.md) — prepaid path when you are **not** on MCA (reselling / pre-conf only)
- [Using P2C](./using_p2c.md) — the stream and how to reply with bundles
- [Tips](../tips.md) — tipping reply bundles is separate from MevShare settlement
