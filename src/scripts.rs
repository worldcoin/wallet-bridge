//! Centralized Lua scripts executed via `redis::Script`. Each constant is the
//! verbatim contents of the matching `.lua` file in `src/scripts/`; co-locate
//! new scripts here so the call sites stay free of inline raw strings.

/// Atomic check-and-insert for the invite-code variant. See `scripts/insert_code.lua`.
pub const INSERT_CODE: &str = include_str!("scripts/insert_code.lua");

/// Atomic one-shot redemption. See `scripts/redeem.lua`.
pub const REDEEM: &str = include_str!("scripts/redeem.lua");
