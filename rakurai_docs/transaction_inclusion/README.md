# Rakurai Transaction Inclusion

Documentation for searchers, block engines, and partners integrating with Rakurai validators.

**Audience:** Transaction landing services, MEV searchers, and partners sending bundles or consuming post-pack confirmations.

---

## 1. Guides

| Guide | Description |
|-------|-------------|
| [Transaction inclusion](./transaction_inclusion.md) | Bundles, virtual priority boost, and post-pack confirmations |
| [Post-pack confirmations](./post_pack_confirmations.md) | gRPC protocol, endpoint configuration, and Admin RPC |
| [Tips FAQ](./rakurai_tip_manager_faqs.md) | Sending tips, prioritization, custom accounts, and distribution |
| [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md) | Record MCA MevShare (post-pack), settle TCA/MCA revenue (`rakurai-revshare`); partners must hold MCA `record_authority` |

---

## 2. Validator operations

For validator-side configuration (Admin RPC, Geyser, binary verification), see [Validators](../validators/README.md).
