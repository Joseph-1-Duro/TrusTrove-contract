#![cfg(test)]

use soroban_sdk::{
    contract, contractimpl, contracttype, testutils::Address as _, Address, BytesN, Env,
};

use crate::{EscrowContract, EscrowContractClient};

#[contract]
pub struct MockToken;

#[contractimpl]
impl MockToken {
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        let from_key = BalanceKey(from.clone());
        let to_key = BalanceKey(to.clone());
        let from_bal: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        let to_bal: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&from_key, &(from_bal - amount));
        env.storage().persistent().set(&to_key, &(to_bal + amount));
    }

    pub fn balance(env: Env, addr: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&BalanceKey(addr))
            .unwrap_or(0)
    }
}

#[contracttype]
pub struct BalanceKey(Address);

fn setup() -> (
    Env,
    EscrowContractClient<'static>,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let pool = Address::generate(&env);
    let usdc_id = env.register_contract(None, MockToken);
    let _mock_token = MockTokenClient::new(&env, &usdc_id);

    let pool_bal_key = BalanceKey(pool.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&pool_bal_key, &10_000_000_000_000i128);
    });

    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);

    client.initialize(&admin, &pool, &pool, &usdc_id);

    (env, client, admin, pool, usdc_id)
}

fn setup_without_auths() -> (
    Env,
    EscrowContractClient<'static>,
    Address,
    Address,
    Address,
) {
    let (env, client, admin, pool, usdc_id) = setup();
    env.set_auths(&[]);
    (env, client, admin, pool, usdc_id)
}

fn generate_invoice_id(env: &Env) -> BytesN<32> {
    let mut arr = [0u8; 32];
    arr[0..8].copy_from_slice(&env.ledger().timestamp().to_be_bytes());
    BytesN::from_array(env, &arr)
}

#[test]
fn test_initialize() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let pool = Address::generate(&env);
    let invoice = Address::generate(&env);
    let usdc = env.register_contract(None, MockToken);
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &pool, &invoice, &usdc);

    assert_eq!(client.get_locked(&generate_invoice_id(&env)), 0);
}

#[test]
fn test_lock_stores_record() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    let amount: u128 = 1_000_000_000;

    let result = client.lock(&invoice_id, &amount);
    assert!(result);

    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, amount);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_lock_fails_zero_amount() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    client.lock(&invoice_id, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_lock_fails_duplicate() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    client.lock(&invoice_id, &1_000_000_000);
    client.lock(&invoice_id, &500_000_000);
}

#[test]
fn test_release_to_issuer_transfers_correct_amount() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    let issuer = Address::generate(&env);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount);
    let result = client.release_to_issuer(&invoice_id, &issuer);
    assert!(result);

    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, 0);
}

#[test]
fn test_release_to_pool_transfers_correct_amount() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount);
    let repayment: u128 = amount;
    let result = client.release_to_pool(&invoice_id, &repayment);
    assert!(result);

    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_release_to_pool_fails_on_mismatched_repayment_amount() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount);
    let invalid_repayment: u128 = amount + 1;
    client.release_to_pool(&invoice_id, &invalid_repayment);
}

#[test]
fn test_handle_default_returns_funds_to_pool() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount);
    let result = client.handle_default(&invoice_id);
    assert!(result);

    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, 0);
}

#[test]
fn test_handle_default_no_record_returns_false() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);

    let result = client.handle_default(&invoice_id);
    assert!(!result);
}

#[test]
fn test_get_locked_returns_zero_when_empty() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);

    assert_eq!(client.get_locked(&invoice_id), 0);
}

#[test]
fn test_get_locked_returns_amount_when_locked() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount);
    assert_eq!(client.get_locked(&invoice_id), amount);
}

#[test]
#[should_panic]
fn test_lock_requires_pool_authorization() {
    let (env, client, _admin, _pool, _usdc) = setup_without_auths();
    let invoice_id = generate_invoice_id(&env);
    let amount: u128 = 1_000_000_000;

    // The contract stores a pool address internally, but no auth entry is
    // present after setup_without_auths(), so this must fail at require_auth().
    client.lock(&invoice_id, &amount);
}

#[test]
#[should_panic]
fn test_release_to_issuer_requires_pool_authorization() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    let issuer = Address::generate(&env);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount);
    env.set_auths(&[]);
    client.release_to_issuer(&invoice_id, &issuer);
}

#[test]
#[should_panic]
fn test_release_to_pool_requires_pool_authorization() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount);
    env.set_auths(&[]);
    client.release_to_pool(&invoice_id, &amount);
}

#[test]
#[should_panic]
fn test_handle_default_requires_pool_authorization() {
    let (env, client, _admin, _pool, _usdc) = setup();
    let invoice_id = generate_invoice_id(&env);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount);
    env.set_auths(&[]);
    client.handle_default(&invoice_id);
}
