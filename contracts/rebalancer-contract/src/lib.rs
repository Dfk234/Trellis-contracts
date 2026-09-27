#![no_std]

mod fee_calculator;

mod slippage_predictor;

mod strategy_executor;

mod logging;

use fee_calculator::calculate_total_fees;
// use logging::log_trade;
use shared::events::emit_action_executed;
use shared::telemetry::{ActorType, TelemetryResult, TelemetryTimer, OP_REBALANCE};
use slippage_predictor::predict_slippage;
use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Env, Symbol, Vec, U256};
use strategy_executor::{execute_strategy, ExecutionSummary, TradeError, TradeStatus, TradeErrorKind};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trade {
    pub asset_pair: (Symbol, Symbol),
    pub amount: u128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionStrategy {
    MinimalCost,
    MinimalTime,
    Balanced,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulationResult {
    pub expected_fees: u128,
    pub expected_slippage: U256,
}

#[contract]
pub struct MultiAssetRebalancer;

#[contractimpl]
impl MultiAssetRebalancer {
    pub fn rebalance(
        env: Env,
        trades: Vec<Trade>,
        strategy: ExecutionStrategy,
        dry_run: bool,
    ) -> SimulationResult {
        let total_fees = calculate_total_fees(&trades);

        // Observability: measure the whole rebalance request, including the
        // execution path, and emit one structured telemetry payload on exit.
        let timer = TelemetryTimer::start_here(&env, OP_REBALANCE, ActorType::Contract);
        let mut telemetry_result = TelemetryResult::Success;

        // Accumulate slippage from each trade
        // U256 cannot be used directly with addition in no_std, so we accumulate as i128
        // then convert to U256 at the end
        let mut total_slippage_bps: i128 = 0;
        for trade in trades.iter() {
            let slippage = predict_slippage(trade.asset_pair.clone(), trade.amount, &env);
            let slippage_bps = slippage.to_i128(&env);
            // Safe saturating add to prevent overflow
            total_slippage_bps = total_slippage_bps.saturating_add(slippage_bps);
        }
        let total_slippage = U256::from_i128(&env, total_slippage_bps);

        if !dry_run {
            match execute_strategy(&env, &strategy, &trades) {
                Ok(_execution) => {}
                // The contract still returns the simulation result to the
                // caller (unchanged behaviour); telemetry records that the
                // execution path itself failed so operators can see it.
                Err(_err) => telemetry_result = TelemetryResult::Failure,
            }
        }

        let result = SimulationResult {
            expected_fees: total_fees,
            expected_slippage: total_slippage,
        };

        emit_action_executed(
            &env,
            symbol_short!("reb"),
            symbol_short!("rebal"),
            &env.current_contract_address(),
            !dry_run,
            env.ledger().timestamp(),
        );

        timer.finish(&env, telemetry_result);

        result
    }
}
