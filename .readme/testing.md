# Testing

## Running Tests

```sh
cargo test -p rustycan
```

77 unit tests + 11 integration tests covering:
- EDS parser
- Node-ID string parsing
- PDO bit extraction
- SDO command-specifier decode
- NMT encode/decode
- COB-ID classifier

## Firmware Type Check

To verify the firmware crate type-checks (requires the embedded target):

```sh
rustup target add thumbv7em-none-eabihf
cd firmware && cargo check --target thumbv7em-none-eabihf
```
