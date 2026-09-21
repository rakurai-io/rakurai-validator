# Revenue settlement

Checklist for settling what you owe on TIN — tips (**TCA**), backrun / MevShare (**MCA**), and prepaid post-pack (**PSA**).

For **detailed CLI flags and examples**, use the CLI docs linked in each section. Download the latest CLI from the [releases](https://github.com/rakurai-io/rakurai_programs/releases/latest).

Revenue flow: [TIN revenue streams](./README.md#2-revenue-streams). Tip mechanics: [Tips](./tips.md).

**Audience:** Traders and landing services tipping for inclusion, and TIN partners on post-pack (**MCA** *or* **PSA**).

| Stream | Account | CLI | What you do |
|--------|---------|-----|-------------|
| Tips (Rakurai tip accounts) | **TCA** | — | Nothing — settlement is automatic |
| Tips (partner tip account) | **TCA** | [`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md) `--revenue-kind Tip` | Transfer after the epoch (interim; prefer Rakurai tip accounts) |
| Backrun / MevShare | **MCA** | [`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md) `--revenue-kind Mev-share` | Record, then transfer (**PSA included** — no separate PSA) |
| Pre-conf / reselling | **PSA** | [`rakurai-p2c`](../rakurai_programs/cli/p2c_subscription.md) | Fund the validator PDA (no record step) |

> [!CAUTION]
> **Settle within 2 epochs**
>
> Partner tip TCA balances, P2C subscriptions, and backrun revenue must be settled within 2 epochs. Otherwise, the related TIN service may stop. Rakurai tip accounts do not need this manual step.

---

## 1. TCA — Tips Collection Account

Tips for scheduling priority settle into a per-service, per-validator **TCA**. Prefer the **Rakurai tip accounts** path — settlement is automatic. A partner tip account is interim only and requires manual settlement each epoch.

### 1.1. Rakurai tip account (default and recommended)

Traders and landing services tip any of [Rakurai’s eight tip accounts](./tips.md#appendix-program-and-tip-account-addresses). **Nothing to settle from the trader or landing-service side.** Tip into a Rakurai tip account and you are done. (**Minimum recommended tip: `0.001 SOL`**)

### 1.2. Partner tip account (interim — prefer Rakurai tip accounts)

Registering your **own tip account** is interim support only. The flow is more complex than Rakurai tip accounts (manually transfer each epoch). **Use [Rakurai tip accounts](./tips.md#appendix-program-and-tip-account-addresses).**

If you still use a partner tip account: traders tip into that account; tips stay with you — Rakurai cannot drain them.

- **60% of the total tip** is used for virtual priority
- Each leader turn the validator **records** that 60% in the TCA (no SOL moves yet)
- After the epoch, **you transfer** the owed SOL into the TCA

#### 1.2.1. Each epoch

- Wait for the epoch to end
- Use the [`rakurai-revshare` CLI](https://github.com/rakurai-io/rakurai_programs/releases/latest)
- Inspect: `get-all-accounts` (`--revenue-kind Tip`) — view any pending record
- Settle: `transfer-all` (`--dry-run` first if needed) — settle the owed revenue

Detailed commands: [Tip and MevShare Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md). Registration: [custom tip accounts](./tips.md#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts).

---

## 2. MCA — MevShare

For **backrun / MevShare**, settle through the **MCA**. **PSA is included** — do not fund a separate PSA. You receive leader-time **and** TPU streams. Settlement is **self-reported** and you hold the MCA `record_authority`.

- Backrun bundles using **leader-time P2C source transactions** get an additional **20% virtual priority boost**
- Share **60-70%** of backrun / MevShare revenue through TIN
- Expected baseline: **60-70 SOL / month / 1M stake**
- Use your own **RSMS / revenue recognition / attribution system** so you can provide an audit report on Rakurai / validator request
- The **TPU** P2C stream is part of the MCA path (PSA / reselling is leader-time only)

### 2.1. Each epoch

- Wait for the epoch to end
- Use the [`rakurai-revshare` CLI](https://github.com/rakurai-io/rakurai_programs/releases/latest)
- Record: `record-revenue` **once** per validator (`--revenue-kind Mev-share`, `record_authority` keypair) — record only, no SOL moves
- Inspect: `get-all-accounts` (`--revenue-kind Mev-share`) — view any pending record
- Settle: `transfer-all` (`--dry-run` first if needed) — settle the owed revenue

> [!WARNING]
> **Record once per validator per epoch**
>
> `record-revenue` **adds** to the epoch entry. Running it twice books twice what you owe — check with `get-all-accounts` before recording again.

Detailed commands: [Tip and MevShare Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md).

---

## 3. PSA — P2C Subscription (pre-conf / reselling)

For **pre-conf / reselling**, keep a **PSA** funded per service name and validator vote. The account model matches backrun RevShare (one PDA per validator). **There is no record step** — transfer SOL into the validator PDA only. **Any wallet may fund** the PDA; no whitelisting is required.

If you use the **MCA** backrun path instead, **PSA is included** — do not fund a separate PSA.

- If you are **selling P2C to searchers**, share **60-70%** of that subscription revenue through TIN
- Expected baseline: **$50 / 1M txn updates** (dollar equivalent amount in SOL)
- Use your own **RSMS / revenue recognition / attribution system** so you can provide an audit report on Rakurai / validator request

### 3.1. Each epoch

- Use the [`rakurai-p2c` CLI](https://github.com/rakurai-io/rakurai_programs/releases/latest)
- Fund **before** the epoch ends: `fund` (one validator vote) or `fund-all` (`--dry-run` first if needed) — or `solana transfer` to the PDA
- Settle / top up: Use `fund` or `fund-all` to clear shortfalls so the stream stays active

> [!NOTE]
> **Suspended stops delivery**
>
> Status `Suspended` means the stream has stopped until the shortfall is cleared. Updates missed while suspended are not replayed.

Detailed commands: [P2C Subscription CLI](../rakurai_programs/cli/p2c_subscription.md).
