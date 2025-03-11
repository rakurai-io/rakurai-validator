# Rakurai Agave Validator

This repository contains `agave-validator`, a Rakurai-enhanced Jito-Solana client optimized for high block rewards and higher TPS.

## How to Build

- Download the scheduler binary from latest release.
```bash
# Clone repo using this command
git clone git@github.com:rakurai-io/rakurai-validator.git -b release/v2.1.11-rakurai --recurse-submodules
cd rakurai-validator

# untar the downlaoded scheduler binary
mkdir -p ./target/release/
tar -xvzf ./rakurai-scheduler.tar.gz -C ./target/release/

# build the executable using the following command 
cargo build --release --features build_validator

# export the absolue path of the scheduler binary (add this command in validator.sh script if you are using one)
export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:<path_to_rakurai-validator>/target/release 
```

## How to Run

- Ensure your production deployment scripts are running the correct version of `agave-validator` by updating the executable path to the downloaded latest release
- Run your validator along with all the standard [cli arguments](#CLI-Arguments)

#### Note: **`--relayer-url`** argument exists in Jito-Agave but is **not supported** in Rakurai Validator

## CLI-Arguments
For a full list of supported arguments, refer to:
- **Agave Validator arguments**: [Solana Validator Setup Guide](#)  
- **Jito-Solana arguments**: [Jito Solana Command-Line Arguments](#)  
