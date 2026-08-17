# Rakurai Geyser — Guide

Build and run Geyser plugins compatible with the Rakurai validator client.

**Audience:** Validator operators running third-party Geyser plugins with Rakurai.

---

## 1. Overview

The struct layout and padding of the **Rakurai Validator** are slightly different from the standard **Agave/Solana validator**. Because of this, you cannot directly run any Geyser with the Rakurai validator and vice versa. To run any Geyser with Rakurai Validator, you must use the Geyser-related crates from the Rakurai repository and build your Geyser with them.

A sample Geyser plugin binary is provided in this repository: **[spark-geyser](../../spark-geyser/README.md)**.

**Note:** Running a standard Geyser without applying Rakurai-compatible patches **may crash your node or cause undefined behavior**.

---

## 2. Required patch

Apply the following patch to your Geyser's top-level `Cargo.toml`:

```toml
[patch.crates-io]
agave-geyser-plugin-interface = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "agave-geyser-plugin-interface", tag = "release/v3.1.8-rakurai.0" }
solana-account-decoder = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "solana-account-decoder", tag = "release/v3.1.8-rakurai.0" }
solana-transaction-context = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "solana-transaction-context", tag = "release/v3.1.8-rakurai.0" }
solana-transaction-status = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "solana-transaction-status", tag = "release/v3.1.8-rakurai.0" }

[workspace.lints.rust]
deprecated = "allow"
```

**Note:** The release tag in the patch must exactly match the Rakurai validator version. If the versions do not match, the validator may crash due to struct/ABI mismatch.

---

## 3. Build steps

After applying the patch:

```bash
cargo clean
cargo build --release
```

Then run your Geyser plugin as usual.

For a reference implementation and configuration, see the [spark-geyser](../../spark-geyser/README.md).
