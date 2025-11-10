# Rakurai Tip Manager Program

A Solana smart contract for managing tips sent to validators. The program maintains **eight tip accounts** to reduce write-lock contention and automatically splits tips between the validator's tip receiver account and the client commission account.

➤ For more details, refer to the [IDL file](./idl/rakurai_tip_manager.json).

---

## 1. Deployed program ID

### Mainnet

- **Tip Manager program:** [rKtiPTD7WuCdEEQ2JXWgAmZHHL9iZLc3niCXwtS7wSH](https://solscan.io/account/rKtiPTD7WuCdEEQ2JXWgAmZHHL9iZLc3niCXwtS7wSH)
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

- **Tip Manager program:** [4qRZaFzf7MvgfBTCP9grb69cCST8UmKHPtkpGAgkJosD](https://solscan.io/account/4qRZaFzf7MvgfBTCP9grb69cCST8UmKHPtkpGAgkJosD?cluster=testnet)
- **Tip accounts:**
  - `3ahyXyni1jLj8kJ13VgGEFDJzB374dgQW273nJSg8cdm`
  - `3aebD4TAn1somZfiaKRrMypUfmbDzT7XMVWRM5TFHuKW`
  - `Hm4LFyTAbrgH4eejYmNXQJ9oejQyq8frD2qeJbmkCAWR`
  - `AffPqNJ8jSrFGgfiouVfXcra1Vd6gHUjNhpoL8uW8dY5`
  - `9Z4pSxRZzE1T2e6587yzMWtvo8RHKW3R5Rb2FcprUPz`
  - `J2JdwcRrxWyCHKrgi2ipwCFXK2oRSgzPN4P7Q6Kz9XZ9`
  - `DscP7KHpAvfnboSKEQ5KEcwuFuRWn6MTjKYYTftuqY6z`
  - `Ur14r1oNyLvYeFLngGoEwYV4zwFVcui72vJqAavDXhZ`

---

## 2. How it works

The Rakurai Tip Manager Program uses a **singleton configuration account** (`TipManagerConfigAccount`) that controls:

- The **validator tip receiver account** — where validator tips are sent (rotated on each drain to the Rakurai TCA / TCAV1)
- The **client commission account** / **bps** — used by `change_client` and by `change_tip_receiver` / `v1`

`change_tip_receiver_v1` / `v2` drain using tip-manager global commission (set by the previous leader), then sync global from the new TCA / TCAV1 for the next leader.

The program maintains **eight separate tip accounts** (PDAs) to minimize account write-lock contention when multiple transactions send tips simultaneously.

---

## 3. Account structure

### 3.1. TipManagerConfigAccount

A singleton PDA that stores the program configuration:

- **Fields:**
  - `authority` — Authorized updater of the config
  - `validator_tip_receiver_account` — Account receiving validator tips
  - `client_commission_account` — Client commission account
  - `client_commission_bps` — Commission in basis points (0–10000)
  - `bumps` — PDA bump seeds for all tip accounts

### 3.2. RakuraiTipAccounts

These accounts are empty state accounts that hold SOL (lamports). When tips are claimed, all lamports above the rent-exempt minimum are drained and distributed.

---

## 4. Tip distribution flow

1. **Users send tips** → any of the eight tip accounts
2. **Validator drains tips** via one of:
   - `change_tip_receiver` / `change_tip_receiver_v1` — **new** tip receiver must be legacy TCA (`REVENUE_SHARE`); records old TCA **or** TCAV1; drain uses TM global commission, then sync from new TCA
   - `change_tip_receiver_v2` — **new** tip receiver must be TCAV1 (`REVENUE_SHARE_V1`); records old TCA **or** TCAV1; drain uses TM global commission, then sync from new TCAV1
3. **Config update** → `validator_tip_receiver_account` set to the new tip receiver PDA; `client_commission_*` synced from new TCA / TCAV1 for the next leader

---

## 5. Integration with reward distribution

| Path | Tip receiver PDA | Init | Drain ix | Record CPI |
|------|------------------|------|----------|------------|
| Legacy (old validators) | `[REVENUE_SHARE, TIP, rakurai, vote]` | `initialize_revenue_share_account` | `change_tip_receiver_v1` | `record_revenue` (or `record_revenue_v1` if previous was TCAV1) |
| V1 (new validators) | `[REVENUE_SHARE_V1, TIP, rakurai, vote]` | `initialize_revenue_share_account_v1` | `change_tip_receiver_v2` | `record_revenue_v1` (or `record_revenue` if previous was legacy TCA) |

Both `v1` and `v2` detect whether **old** is TCA or TCAV1 and CPI the matching record ix, so mixed handoffs (v1→v2→v1…) still ledger.

SDK: `derive_rakurai_tip_collection_address` (legacy), `derive_rakurai_tip_collection_v1_address` (V1).

On `claim_revenue` / `claim_revenue_v1`, Rakurai tip vaults skip commission (tip drain already paid Rakurai). Legacy pays recorded `amount`; V1 pays `transferred_amount`.

For vault layouts see [Reward Distribution — Tip and MevShare](../reward_distribution/README.md#5-tip-and-mevshare-collection-accounts). Partners settling custom-tip TCA balances can use the [Partner Tip and MevShare Revenue Settlement CLI](../../cli/partner_reward_settlement.md) (`rakurai-partner-settle`).

---

## 6. Account lifecycle

- **Tip Accounts:** Remain open indefinitely, accumulating tips until drained
- **Config Account:** Remains open until explicitly closed by the authority
- All accounts preserve rent exemption when tips are drained
