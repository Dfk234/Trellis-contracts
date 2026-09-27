use soroban_sdk::{Env, Symbol, U256};

const BASE_SLIPPAGE_BPS: u128 = 5;
const VOLUME_STEP: u128 = 100_000;
const VOLUME_STEP_BPS: u128 = 1;
const MAX_SLIPPAGE_BPS: u128 = 10_000;

pub fn predict_slippage(asset_pair: (Symbol, Symbol), amount: u128, env: &Env) -> U256 {
    if amount == 0 {
        return U256::from_u32(env, 0);
    }

    let volume_bps = (amount / VOLUME_STEP).saturating_mul(VOLUME_STEP_BPS);
    let pair_penalty = if asset_pair.0 == asset_pair.1 {
        MAX_SLIPPAGE_BPS
    } else {
        0
    };
    let bps = BASE_SLIPPAGE_BPS
        .saturating_add(volume_bps)
        .saturating_add(pair_penalty)
        .min(MAX_SLIPPAGE_BPS);
    U256::from_u32(env, bps as u32)
}
