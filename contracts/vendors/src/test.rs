#![cfg(test)]

extern crate std;

use soroban_sdk::{
    testutils::Address as _,
    token::{StellarAssetClient, TokenClient},
    Address, Env, String, Symbol,
};

use crate::{
    InferenceProvider, InferenceProviderClient, PriceFeed, PriceFeedClient, WeatherOracle,
    WeatherOracleClient,
};

fn sac(env: &Env) -> (Address, Address) {
    let admin = Address::generate(env);
    let token = env.register_stellar_asset_contract_v2(admin);
    (token.address(), Address::generate(env))
}

fn fund(env: &Env, token: &Address, payer: &Address, amount: i128) {
    StellarAssetClient::new(env, token).mint(payer, &amount);
}

#[test]
fn weather_collects_fee_then_returns_report() {
    let env = Env::default();
    env.mock_all_auths();
    let (token, payer) = sac(&env);
    fund(&env, &token, &payer, 1_000);
    let id = env.register(WeatherOracle, ());
    let client = WeatherOracleClient::new(&env, &id);
    let report = client.get_weather(&payer, &token, &String::from_str(&env, "accra"), &50);
    assert_eq!(report.temperature_c, 21);
    assert_eq!(TokenClient::new(&env, &token).balance(&payer), 950);
    assert_eq!(TokenClient::new(&env, &token).balance(&id), 50);
}

#[test]
fn price_feed_collects_fee() {
    let env = Env::default();
    env.mock_all_auths();
    let (token, payer) = sac(&env);
    fund(&env, &token, &payer, 500);
    let id = env.register(PriceFeed, ());
    let client = PriceFeedClient::new(&env, &id);
    let px = client.get_price(&payer, &token, &Symbol::new(&env, "XLM"), &100);
    assert_eq!(px, 12_000);
    assert_eq!(TokenClient::new(&env, &token).balance(&payer), 400);
    assert_eq!(TokenClient::new(&env, &token).balance(&id), 100);
}

#[test]
fn inference_rejects_empty_prompt_without_transfer() {
    let env = Env::default();
    env.mock_all_auths();
    let (token, payer) = sac(&env);
    fund(&env, &token, &payer, 800);
    let id = env.register(InferenceProvider, ());
    let client = InferenceProviderClient::new(&env, &id);
    assert!(client
        .try_run_inference(&payer, &token, &String::from_str(&env, ""), &500)
        .is_err());
    assert_eq!(TokenClient::new(&env, &token).balance(&payer), 800);
}

#[test]
fn inference_collects_fee() {
    let env = Env::default();
    env.mock_all_auths();
    let (token, payer) = sac(&env);
    fund(&env, &token, &payer, 800);
    let id = env.register(InferenceProvider, ());
    let client = InferenceProviderClient::new(&env, &id);
    let out = client.run_inference(&payer, &token, &String::from_str(&env, "hello"), &500);
    assert_eq!(out, String::from_str(&env, "inference:ok"));
    assert_eq!(TokenClient::new(&env, &token).balance(&payer), 300);
}

#[test]
fn zero_fee_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (token, payer) = sac(&env);
    fund(&env, &token, &payer, 100);
    let id = env.register(WeatherOracle, ());
    let client = WeatherOracleClient::new(&env, &id);
    assert!(client
        .try_get_weather(&payer, &token, &String::from_str(&env, "accra"), &0)
        .is_err());
    assert_eq!(TokenClient::new(&env, &token).balance(&payer), 100);
}
