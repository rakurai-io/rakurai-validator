# Running Geyser with Rakurai Validator

## Overview

The struct layout and padding of the **Rakurai Validator** are slightly different from the standard **Agave/Solana validator**. Because of this, you cannot directly run any Geyser with the Rakurai validator and vice-verse. To run any Geyser with Rakurai Validator, you must use the Geyser-related crates from the Rakurai repository and build your Geyser with them.

---

## Required Patch

Apply the following patch to your Geyser’s top-level `Cargo.toml`:

```toml
[patch.crates-io]
agave-geyser-plugin-interface = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "agave-geyser-plugin-interface", tag = "release/v3.1.8-rakurai.0" }
solana-account-decoder = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "solana-account-decoder", tag = "release/v3.1.8-rakurai.0" }
solana-transaction-context = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "solana-transaction-context", tag = "release/v3.1.8-rakurai.0" }
solana-transaction-status = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "solana-transaction-status", tag = "release/v3.1.8-rakurai.0" }

[workspace.lints.rust]
deprecated = "allow"
```
## Note
- The release tag in the patch must exactly match the Rakurai validator version. If the versions do not match, the validator may crash due to struct/ABI mismatch.


## Build Steps

After applying the patch:

```bash
cargo clean
cargo build --release
```

Then run your Geyser plugin as usual.

---
