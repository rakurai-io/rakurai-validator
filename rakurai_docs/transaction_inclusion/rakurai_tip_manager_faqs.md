# Rakurai Tips — FAQ

Frequently asked questions about how tips work on Rakurai validators: sending tips, transaction prioritization, and distribution.

**Audience:** Searchers, transaction inclusion services, and traders sending tips to Rakurai validators.

**References:**

- [Rakurai Tip Manager program](../rakurai_programs/programs/rakurai_tip_manager/README.md)
- [RevenueShareAccount / RevenueShareAccountV1 structure](../rakurai_programs/programs/reward_distribution/README.md#56-revenueshareaccount-revenueshareaccountv1-structure) (TCA and MCA account layout in the Reward Distribution program)
- [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md) (`rakurai-revshare` — inspect and settle TCA/MCA)

---

## 1. What are tips?

Tips help traders and searchers land bundles (or transactions) on Rakurai nodes. Bundles (or transactions) that pay higher tips are generally prioritized over those that pay lower tips.

**Note:** Tips are not a replacement for priority fees. The Rakurai implementation does not allow cannibalization of priority fees.

---

## 2. How are tips transferred?

### 2.1. Sending a tip

Tips are plain **SOL transfers** to one of the eight [Rakurai tip accounts](#appendix-program-and-tip-account-addresses). No special program instruction is required to *send* a tip — include a `SystemProgram.transfer` in your transaction (or bundle) that moves lamports from your wallet to any of those tip PDAs.

**Why eight accounts?** Write-lock contention. If every tipper hit a single account, transactions would serialize on that account lock. Eight parallel tip accounts let many tippers land simultaneously.

---

## 3. How do tips prioritize my transaction?

When your transaction includes a SOL transfer to a Rakurai tip account, that transfer amount is the **tip signal** the scheduler uses to prioritize your transaction (or bundle).

### 3.1. What you should do

1. **Include the tip inside your transaction** (or bundle) as a `SystemProgram.transfer` to one of the eight Rakurai tip accounts.
2. **Higher tip → higher scheduling priority** when the current slot leader is running the Rakurai scheduler. The scheduler weighs reward (tip + fees) relative to estimated compute cost, similar to Solana's standard priority formula: transactions that pay more per compute unit are prioritized.

### 3.2. What tips do *not* do

1. A tip is not a replacement for regular **priority fees**. If a transaction has very low priority fees and a very high tip, tip-based prioritization will be less significant.
2. Tips are independent of Solana's base transaction fee and prioritization fee — they are an additional incentive for validators.

---

## 4. Can I use my own tip account instead of Rakurai's eight accounts?

The preferred mechanism is to send tips to Rakurai's tip accounts. However, for the time being, if you need to receive tips in your own account(s), contact the **Rakurai team** on Slack or [Telegram](https://t.me/rakurai_official) to configure your **custom tip accounts**.

### 4.1. How it works

1. **Register your account** — provide your tip account addresses (e.g., `ABC...DEF`) to the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) and agree on a commission percentage you will share with Rakurai (e.g., 30%).
2. **Rakurai adds your account to the validator flow** — the team configures the system so that tips sent to your custom account are recognized by the Rakurai scheduler. Validators control which tip accounts are effective through Admin RPC.
3. **Prioritization** — when someone tips your custom account, the agreed Rakurai share (e.g., 30%) is used by the scheduler to prioritize the transaction, the same way standard Rakurai tip accounts work.
4. **Settlement** — after the epoch ends, settle the agreed Rakurai share into the validator's **[Tips Collection Account (TCA)](../rakurai_programs/programs/reward_distribution/README.md#51-why-a-tips-collection-account-tca)** using the [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md) (`transfer --revenue-kind Tip`), following the same [tip distribution flow](#6-how-are-tips-distributed).

---

## 5. How are tips accumulated?

On **every Rakurai leader turn**, the validator client automatically submits a tip-receiver claim transaction (`change_tip_receiver` / `change_tip_receiver_v1` / `change_tip_receiver_v2`, depending on release) that:

1. Drains all eight tip accounts (lamports above rent exemption from the **previous leader period**).
2. Transfers Rakurai's commission percentage to the Rakurai commission account.
3. Transfers the remaining share to the validator's **[Tips Collection Account (TCA)](../rakurai_programs/programs/reward_distribution/README.md#51-why-a-tips-collection-account-tca)**.

---

## 6. How are tips distributed?

Tip distribution happens in two stages. The mechanism differs depending on whether tips land in **Rakurai's tip accounts** or in an **external custom tip account**.

Both TCA and MCA use the on-chain **[RevenueShareAccount / RevenueShareAccountV1 structure](../rakurai_programs/programs/reward_distribution/README.md#56-revenueshareaccount-revenueshareaccountv1-structure)**; recording timing differs between them (see [leader-turn](#61-leader-turn-stage-every-leader-turn) and [post-epoch](#62-post-epoch-stage-after-epoch-ends) stages). Partners can inspect pending records and settle with the [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md).

### 6.1. Leader-turn stage (every leader turn)

On every Rakurai leader turn, the claim transaction processes tips from the previous leader period:

- **Rakurai's eight tip accounts** — drained directly. Rakurai's share is transferred to the **Rakurai commission account** (the commission percentage is set on-chain via the [Tip Manager config account](https://solscan.io/account/rKtiPTD7WuCdEEQ2JXWgAmZHHL9iZLc3niCXwtS7wSH?accountName=428560b549b70266#accountsData)), and the remaining share is transferred into the validator's **[Tips Collection Account (TCA)](../rakurai_programs/programs/reward_distribution/README.md#51-why-a-tips-collection-account-tca)**.
- **External custom tip accounts** — cannot be drained (Rakurai does not control them), so the attributed amount is only **recorded on-chain** in the relevant per-validator, per-service [TCA ledger](../rakurai_programs/programs/reward_distribution/README.md#56-revenueshareaccount-revenueshareaccountv1-structure) each leader turn. No lamports move at this stage.
- **MevShare (MCA)** — nothing happens on-chain during leader turns. Post-pack / MEV-share revenue stays in the searcher or transaction inclusion service's own accounts until the epoch ends.

### 6.2. Post-epoch stage (after epoch ends)

After the epoch ends, TCA and MCA revenue is distributed following the same [Tip and MevShare distribution flow](../rakurai_programs/programs/reward_distribution/README.md#53-how-tip-and-mevshare-are-distributed):

- **Rakurai's eight tip accounts** — the Rakurai tip balance in the relevant [TCA](../rakurai_programs/programs/reward_distribution/README.md#51-why-a-tips-collection-account-tca) is transferred to the validator identity account (with the option to convert it into block rewards).
- **External custom tip accounts** — the external tip account holder must first **settle** their agreed share into the relevant [TCA](../rakurai_programs/programs/reward_distribution/README.md#51-why-a-tips-collection-account-tca) (use [Partner CLI `transfer --revenue-kind Tip`](../rakurai_programs/cli/partner_reward_settlement.md#35-transfer)). Once settled, commission is deducted (credited to Rakurai's account) and the remaining share is transferred to the validator identity account (with the option to convert it into block rewards).
- **MevShare (MCA)** — when you start using post-pack, Rakurai creates your MCA; you must hold its **`record_authority`**. After the epoch ends, the service **records** the owed revenue on the [MCA](../rakurai_programs/programs/reward_distribution/README.md#52-why-a-mevshare-collection-account-mca) **once** ([Partner CLI `record-revenue`](../rakurai_programs/cli/partner_reward_settlement.md#34-record-revenue-mca-only)), then **settles** it ([Partner CLI `transfer --revenue-kind Mev-share`](../rakurai_programs/cli/partner_reward_settlement.md#35-transfer)). Once settled, commission is deducted and the validator share is credited. For the full post-pack flow, see [MEV revenue sharing](./post_pack_confirmations.md#4-mev-revenue-sharing).

If a holder does not settle within **2 epochs**, their account stops being used for prioritization starting from the next epoch (two-epoch grace period).

---

## 7. Common questions

**Q7.1: Do I need to interact with the Tip Manager program to send a tip?**  
A: No. A normal SOL transfer to any of the eight tip accounts is sufficient.

**Q7.2: Can I tip multiple accounts in one transaction?**  
A: Yes, but one transfer to one tip account is typical. Multiple transfers to the same account in one transaction still accumulate correctly.

**Q7.3: What happens if tips are never claimed?**  
A: They remain in the tip PDAs until the next Rakurai validator leader turn, when the validator runs the tip-receiver claim (`change_tip_receiver` / `v1` / `v2`).

**Q7.4: Where does the validator's tip share go?**  
A: The validator's share is transferred to the epoch-specific **[TCA](../rakurai_programs/programs/reward_distribution/README.md#51-why-a-tips-collection-account-tca)** on every leader turn. After the epoch ends, it is transferred to the validator identity account.

**Q7.5: How is this different from a priority fee?**  
A: Priority fees are paid through Solana's native fee mechanism. Rakurai tips are direct SOL transfers to tip PDAs that are used by the Rakurai scheduler to prioritize traders' and searchers' bundles. Bundles with higher tips are generally scheduled before bundles with lower tips.

**Q7.6: Can I use a custom tip account instead of the eight Rakurai accounts?**  
A: For the time being, yes. Contact the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) to register your own tip account and configure a commission share. See [custom tip accounts](#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts).

**Q7.7: Where can I see TCA, MCA, and Tip Manager account layouts?**  
A: TCA and MCA use the [RevenueShareAccount / RevenueShareAccountV1 structure](../rakurai_programs/programs/reward_distribution/README.md#56-revenueshareaccount-revenueshareaccountv1-structure) in the Reward Distribution program. To inspect pending amounts and settle, use the [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md) (`rakurai-revshare`). For Tip Manager instructions and accounts, see the [Tip Manager program guide](../rakurai_programs/programs/rakurai_tip_manager/README.md).

---

## Appendix: Program and tip account addresses

### Mainnet

- **Tip Manager program:** `rKtiPTD7WuCdEEQ2JXWgAmZHHL9iZLc3niCXwtS7wSH`
- **Tip accounts:**
  - `BjqjPHFmwr19YFmkH8CMNJFbj1wzX9k9ngr4am2nQEdq`
  - `9CNKnAqJgLA4pL6KByzhhdY4mKoQP5wcPdhJgnvvi5Ve`
  - `5wy4C2VMFhHE4i8PWKNS1K4SV275zjNwhLwfKBwajrro`
  - `AgMdA97pk2i2Ry4YQ4iVPNrRiFhcH3x3ARUCiQGt3vJG`
  - `4Qf8JFV5vmpADXNouoJriQ9KiniT5DENrz9JM2mKGH9m`
  - `AuFAFzbzE9dzMajy4RNdyJZBTskeiuJQqT2wd9xoGSRD`
  - `8aLaHz8595MAvgxKoBJEyZmDfqQp8CorezFGYnC7CPjy`
  - `H6hyJo6rpBmwHbvVuWCEHExJ2bE4rcn1hTPeiBtypus4`

### Testnet

- **Tip Manager program:** `4qRZaFzf7MvgfBTCP9grb69cCST8UmKHPtkpGAgkJosD`
- **Tip accounts:**
  - `3ahyXyni1jLj8kJ13VgGEFDJzB374dgQW273nJSg8cdm`
  - `3aebD4TAn1somZfiaKRrMypUfmbDzT7XMVWRM5TFHuKW`
  - `Hm4LFyTAbrgH4eejYmNXQJ9oejQyq8frD2qeJbmkCAWR`
  - `AffPqNJ8jSrFGgfiouVfXcra1Vd6gHUjNhpoL8uW8dY5`
  - `9Z4pSxRZzE1T2e6587yzMWtvo8RHKW3R5Rb2FcprUPz`
  - `J2JdwcRrxWyCHKrgi2ipwCFXK2oRSgzPN4P7Q6Kz9XZ9`
  - `DscP7KHpAvfnboSKEQ5KEcwuFuRWn6MTjKYYTftuqY6z`
  - `Ur14r1oNyLvYeFLngGoEwYV4zwFVcui72vJqAavDXhZ`
