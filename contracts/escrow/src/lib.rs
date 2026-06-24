#![no_std]

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, BytesN, Env, Vec};

mod errors;
mod events;
mod test;
mod types;

pub use errors::*;
pub use types::*;

#[contract]
pub struct EscrowContract;

#[contractimpl]
impl EscrowContract {
    pub fn initialize(
        env: Env,
        admin: Address,
        pool_contract: Address,
        invoice_contract: Address,
        usdc_asset: Address,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, EscrowError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::PoolContract, &pool_contract);
        env.storage()
            .instance()
            .set(&DataKey::InvoiceContract, &invoice_contract);
        env.storage()
            .instance()
            .set(&DataKey::UsdcAsset, &usdc_asset);
        Self::extend_instance_ttl(&env);
    }

    pub fn lock(env: Env, invoice_id: BytesN<32>, amount: u128) -> bool {
        let pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::PoolContract)
            .unwrap();
        pool.require_auth();

        if amount == 0 {
            panic_with_error!(&env, EscrowError::InvalidAmount);
        }

        let key = DataKey::Locked(invoice_id.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, EscrowError::AlreadyLocked);
        }

        let usdc_id: Address = env.storage().instance().get(&DataKey::UsdcAsset).unwrap();
        let usdc = token::Client::new(&env, &usdc_id);
        usdc.transfer(&pool, &env.current_contract_address(), &(amount as i128));

        let record = EscrowRecord {
            invoice_id: invoice_id.clone(),
            amount,
            locked_at: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&key, &record);
        env.storage().persistent().extend_ttl(&key, 100, 2_000_000);
        Self::append_history(&env, &invoice_id, EscrowAction::Locked, amount);
        Self::extend_instance_ttl(&env);
        events::funds_locked(&env, &invoice_id, amount);

        true
    }

    pub fn release_to_issuer(env: Env, invoice_id: BytesN<32>, issuer: Address) -> bool {
        let pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::PoolContract)
            .unwrap();
        pool.require_auth();

        let key = DataKey::Locked(invoice_id.clone());
        let record: EscrowRecord = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotFound));

        let usdc_id: Address = env.storage().instance().get(&DataKey::UsdcAsset).unwrap();
        let usdc = token::Client::new(&env, &usdc_id);
        usdc.transfer(
            &env.current_contract_address(),
            &issuer,
            &(record.amount as i128),
        );

        Self::append_history(
            &env,
            &invoice_id,
            EscrowAction::ReleasedToIssuer,
            record.amount,
        );
        env.storage().persistent().remove(&key);
        Self::extend_instance_ttl(&env);
        events::released_to_issuer(&env, &invoice_id, &issuer, record.amount);
        true
    }

    pub fn release_to_pool(env: Env, invoice_id: BytesN<32>, repayment_amount: u128) -> bool {
        let pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::PoolContract)
            .unwrap();
        pool.require_auth();

        let key = DataKey::Locked(invoice_id.clone());
        let record: EscrowRecord = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotFound));

        if repayment_amount != record.amount {
            panic_with_error!(&env, EscrowError::InvalidAmount);
        }

        let usdc_id: Address = env.storage().instance().get(&DataKey::UsdcAsset).unwrap();
        let usdc = token::Client::new(&env, &usdc_id);
        usdc.transfer(
            &env.current_contract_address(),
            &pool,
            &(repayment_amount as i128),
        );

        Self::append_history(
            &env,
            &invoice_id,
            EscrowAction::ReleasedToPool,
            repayment_amount,
        );
        env.storage().persistent().remove(&key);
        Self::extend_instance_ttl(&env);
        events::released_to_pool(&env, &invoice_id, &pool, repayment_amount);
        true
    }

    pub fn handle_default(env: Env, invoice_id: BytesN<32>) -> bool {
        let key = DataKey::Locked(invoice_id.clone());
        if !env.storage().persistent().has(&key) {
            return false;
        }

        let _admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        let pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::PoolContract)
            .unwrap();

        // Try admin auth first; if that's not the caller, try pool
        // In Soroban, we can't easily catch require_auth failures,
        // so we require the pool to auth (the common case)
        pool.require_auth();

        let record: EscrowRecord = env.storage().persistent().get(&key).unwrap();
        let usdc_id: Address = env.storage().instance().get(&DataKey::UsdcAsset).unwrap();
        let usdc = token::Client::new(&env, &usdc_id);
        usdc.transfer(
            &env.current_contract_address(),
            &pool,
            &(record.amount as i128),
        );

        Self::append_history(
            &env,
            &invoice_id,
            EscrowAction::DefaultHandled,
            record.amount,
        );
        env.storage().persistent().remove(&key);
        Self::extend_instance_ttl(&env);
        events::default_resolved(&env, &invoice_id, &pool, record.amount);
        true
    }

    pub fn get_locked(env: Env, invoice_id: BytesN<32>) -> u128 {
        env.storage()
            .persistent()
            .get::<_, EscrowRecord>(&DataKey::Locked(invoice_id))
            .map(|r| r.amount)
            .unwrap_or(0)
    }

    pub fn get_history(env: Env, invoice_id: BytesN<32>) -> Vec<EscrowEvent> {
        let key = DataKey::History(invoice_id);
        env.storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env))
    }

    fn append_history(env: &Env, invoice_id: &BytesN<32>, action: EscrowAction, amount: u128) {
        let key = DataKey::History(invoice_id.clone());
        let mut history: Vec<EscrowEvent> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(env));
        history.push_back(EscrowEvent {
            invoice_id: invoice_id.clone(),
            action,
            amount,
            timestamp: env.ledger().timestamp(),
        });
        env.storage().persistent().set(&key, &history);
        env.storage().persistent().extend_ttl(&key, 100, 2_000_000);
    }

    fn extend_instance_ttl(env: &Env) {
        env.storage().instance().extend_ttl(100, 2_000_000);
    }
}
