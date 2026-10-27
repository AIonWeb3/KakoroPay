//! Paid vendor contracts: weather, asset price, and inference.
//! Each entrypoint pulls a SAC fee from the payer, then returns data.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, token::TokenClient,
    Address, Env, String, Symbol,
};

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum VendorError {
    InvalidFee = 1,
    EmptyPrompt = 2,
}

fn charge(env: &Env, payer: &Address, token: &Address, fee: i128) {
    if fee <= 0 {
        panic_with_error!(env, VendorError::InvalidFee);
    }
    payer.require_auth();
    TokenClient::new(env, token).transfer(payer, &env.current_contract_address(), &fee);
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherReport {
    pub location: String,
    pub temperature_c: i32,
    pub condition: Symbol,
}

#[contract]
pub struct WeatherOracle;

#[contractimpl]
impl WeatherOracle {
    pub fn get_weather(
        env: Env,
        payer: Address,
        token: Address,
        location: String,
        fee: i128,
    ) -> WeatherReport {
        charge(&env, &payer, &token, fee);
        WeatherReport {
            location,
            temperature_c: 21,
            condition: Symbol::new(&env, "clear"),
        }
    }
}

#[contract]
pub struct PriceFeed;

#[contractimpl]
impl PriceFeed {
    /// Returns a quoted price in stroops after the SAC fee is collected.
    pub fn get_price(env: Env, payer: Address, token: Address, asset: Symbol, fee: i128) -> i128 {
        charge(&env, &payer, &token, fee);
        if asset == Symbol::new(&env, "BTC") {
            6_500_000_000
        } else if asset == Symbol::new(&env, "ETH") {
            320_000_000
        } else if asset == Symbol::new(&env, "XLM") {
            12_000
        } else {
            1_000
        }
    }
}

#[contract]
pub struct InferenceProvider;

#[contractimpl]
impl InferenceProvider {
    pub fn run_inference(
        env: Env,
        payer: Address,
        token: Address,
        prompt: String,
        fee: i128,
    ) -> String {
        if prompt.len() == 0 {
            panic_with_error!(&env, VendorError::EmptyPrompt);
        }
        charge(&env, &payer, &token, fee);
        String::from_str(&env, "inference:ok")
    }
}

#[cfg(test)]
mod test;
