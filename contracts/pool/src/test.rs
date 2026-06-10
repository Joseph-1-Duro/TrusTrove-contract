#![cfg(test)]

use soroban_sdk::{
    contract, contractimpl, contracttype, testutils::Address as _, Address, BytesN, Env,
};

use crate::{LPPosition, PoolContract, PoolContractClient, PoolStats};

use trusttrove_escrow::{EscrowContract as RealEscrow, EscrowContractClient as RealEscrowClient};
use trusttrove_invoice::{
    InvoiceContract as RealInvoice, InvoiceContractClient as RealInvoiceClient,
};

// --------------- Mock Registry ---------------

#[contract]
pub struct MockRegistry;

#[contractimpl]
impl MockRegistry {
    pub fn is_verified(env: Env, address: Address) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&RegKey(address))
            .unwrap_or(false)
    }

    pub fn register(env: Env, address: Address) {
        env.storage()
            .persistent()
            .set(&RegKey(address.clone()), &true);
        env.storage()
            .persistent()
            .extend_ttl(&RegKey(address), 100, 2_000_000);
    }
}

#[contracttype]
pub struct RegKey(Address);

// --------------- Mock Token ---------------

#[contract]
pub struct MockToken;

#[contractimpl]
impl MockToken {
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        let from_key = TKey(from.clone());
        let to_key = TKey(to.clone());
        let from_bal: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        let to_bal: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&from_key, &(from_bal - amount));
        env.storage().persistent().set(&to_key, &(to_bal + amount));
    }

    pub fn balance(env: Env, addr: Address) -> i128 {
        env.storage().persistent().get(&TKey(addr)).unwrap_or(0)
    }
}

#[contracttype]
pub struct TKey(Address);

struct TestEnv {
    env: Env,
    pool: PoolContractClient<'static>,
    invoice: RealInvoiceClient<'static>,
    invoice_id: Address,
    escrow_id: Address,
    usdc_id: Address,
    admin: Address,
    issuer: Address,
    buyer: Address,
    lp: Address,
}

fn setup() -> TestEnv {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let lp = Address::generate(&env);

    let registry_id = env.register_contract(None, MockRegistry);
    let registry = MockRegistryClient::new(&env, &registry_id);
    registry.register(&issuer);
    registry.register(&buyer);

    let usdc_id = env.register_contract(None, MockToken);

    let lp_bal_key = TKey(lp.clone());
    env.storage()
        .persistent()
        .set(&lp_bal_key, &100_000_000_000_000i128);
    let buyer_bal_key = TKey(buyer.clone());
    env.storage()
        .persistent()
        .set(&buyer_bal_key, &100_000_000_000_000i128);

    let invoice_id = env.register_contract(None, RealInvoice);
    let escrow_id = env.register_contract(None, RealEscrow);
    let pool_id = env.register_contract(None, PoolContract);

    let invoice = RealInvoiceClient::new(&env, &invoice_id);
    invoice.initialize(&admin, &registry_id);

    let pool = PoolContractClient::new(&env, &pool_id);
    pool.initialize(&admin, &invoice_id, &escrow_id, &usdc_id);

    let escrow = RealEscrowClient::new(&env, &escrow_id);
    escrow.initialize(&admin, &pool_id, &invoice_id, &usdc_id);

    invoice.set_pool_contract(&pool_id);

    TestEnv {
        env,
        pool,
        invoice,
        invoice_id,
        escrow_id,
        usdc_id,
        admin,
        issuer,
        buyer,
        lp,
    }
}

fn create_and_list(te: &TestEnv) -> BytesN<32> {
    let due_date = te.env.ledger().timestamp() + 86400;
    let invoice_id = te
        .invoice
        .create(&te.issuer, &te.buyer, &10_000_000_000, &due_date);
    te.invoice.list_for_financing(&invoice_id, &200);
    invoice_id
}

// ============== DEPOSIT TESTS ==============

#[test]
fn test_deposit_issues_correct_shares() {
    let te = setup();
    let shares = te.pool.deposit(&te.lp, &1_000_000_000);
    assert_eq!(shares, 1_000_000_000);
}

#[test]
fn test_first_deposit_is_one_to_one() {
    let te = setup();
    let shares = te.pool.deposit(&te.lp, &5_000_000_000);
    assert_eq!(shares, 5_000_000_000);

    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 5_000_000_000);
    assert_eq!(pos.deposit_count, 1);
}

#[test]
fn test_second_deposit_scales_by_share_price() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    // Second deposit at same price
    let shares = te.pool.deposit(&te.lp, &5_000_000_000);
    assert_eq!(shares, 5_000_000_000);

    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 15_000_000_000);
    assert_eq!(pos.deposit_count, 2);
}

// ============== WITHDRAW TESTS ==============

#[test]
fn test_withdraw_returns_correct_usdc() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    let usdc = te.pool.withdraw(&te.lp, &5_000_000_000);
    assert_eq!(usdc, 5_000_000_000);
}

#[test]
#[should_panic(expected = "PoolError::InvalidAmount")]
fn test_withdraw_zero_shares_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    te.pool.withdraw(&te.lp, &0);
}

#[test]
#[should_panic(expected = "PoolError::InsufficientShares")]
fn test_withdraw_more_than_owned_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    te.pool.withdraw(&te.lp, &20_000_000_000);
}

// ============== FUND INVOICE TESTS ==============

#[test]
fn test_fund_invoice_reduces_available_liquidity() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te);

    let before = te.pool.get_stats();
    let _ = te.pool.fund_invoice(&invoice_id);
    let after = te.pool.get_stats();

    assert_eq!(after.active_invoice_count, 1);
    assert!(after.total_funded > before.total_funded);
    assert!(after.available_liquidity < before.available_liquidity);
}

#[test]
#[should_panic(expected = "PoolError::InsufficientLiquidity")]
fn test_fund_invoice_fails_when_insufficient_liquidity() {
    let te = setup();
    // Don't deposit anything — pool is empty
    let invoice_id = create_and_list(&te);
    te.pool.fund_invoice(&invoice_id);
}

// ============== STATS TESTS ==============

#[test]
fn test_get_stats_initial_state() {
    let te = setup();
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_deposits, 0);
    assert_eq!(stats.total_shares, 0);
    assert_eq!(stats.total_funded, 0);
    assert_eq!(stats.active_invoice_count, 0);
    assert_eq!(stats.available_liquidity, 0);
    assert_eq!(stats.utilization_rate_bps, 0);
}

#[test]
fn test_get_stats_after_deposit() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_deposits, 100_000_000_000);
    assert_eq!(stats.total_shares, 100_000_000_000);
    assert_eq!(stats.available_liquidity, 100_000_000_000);
    assert_eq!(stats.utilization_rate_bps, 0);
}

#[test]
fn test_get_stats_after_funding() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te);
    let _ = te.pool.fund_invoice(&invoice_id);

    let stats = te.pool.get_stats();
    assert!(stats.total_funded > 0);
    assert!(stats.available_liquidity < 100_000_000_000);
    assert_eq!(stats.active_invoice_count, 1);
    assert!(stats.utilization_rate_bps > 0);
}

// ============== LP POSITION TESTS ==============

#[test]
fn test_lp_position_empty() {
    let te = setup();
    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 0);
    assert_eq!(pos.usdc_value, 0);
    assert_eq!(pos.yield_earned, 0);
    assert_eq!(pos.deposit_count, 0);
}

#[test]
fn test_lp_position_after_deposit() {
    let te = setup();
    te.pool.deposit(&te.lp, &50_000_000_000);
    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 50_000_000_000);
    assert_eq!(pos.usdc_value, 50_000_000_000);
    assert_eq!(pos.deposit_count, 1);
}

// ============== UTILIZATION RATE TESTS ==============

#[test]
fn test_utilization_rate_zero_when_no_deposits() {
    let te = setup();
    assert_eq!(te.pool.get_utilization_rate(), 0);
}

#[test]
fn test_utilization_rate_zero_when_no_funding() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    assert_eq!(te.pool.get_utilization_rate(), 0);
}

#[test]
fn test_utilization_rate_after_funding() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te);
    let _ = te.pool.fund_invoice(&invoice_id);
    let rate = te.pool.get_utilization_rate();
    assert!(rate > 0);
    assert!(rate < 10000);
}

// ============== MULTI-LP TESTS ==============

#[test]
fn test_multiple_lps_can_deposit() {
    let te = setup();
    let lp2 = Address::generate(&te.env);
    let lp2_bal_key = TKey(lp2.clone());
    te.env
        .storage()
        .persistent()
        .set(&lp2_bal_key, &100_000_000_000_000i128);

    let s1 = te.pool.deposit(&te.lp, &10_000_000_000);
    let s2 = te.pool.deposit(&lp2, &20_000_000_000);

    assert_eq!(s1, 10_000_000_000);
    assert_eq!(s2, 20_000_000_000);

    let stats = te.pool.get_stats();
    assert_eq!(stats.total_shares, 30_000_000_000);
    assert_eq!(stats.total_deposits, 30_000_000_000);
}
