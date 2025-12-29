## bth-common

This crate contains several things right now:

- Common structs that are used at API boundaries of higher-level crates.
  Putting the structs in `common` instead of in a larger crate allows us to break up
  dependencies, and minimize the amount of code that compiles as `std` and `no_std`.
- Common error types, used at API boundaries. The rationale is similar.
- A hashmap object based on hashbrown.
- A simple LRU cache implementation
- Logging functionality controlled by `log` and `loggers` features.
