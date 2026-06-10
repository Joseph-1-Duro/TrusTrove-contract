#![cfg(test)]

use soroban_sdk::{
    contract, contractimpl, contracttype, testutils::Address as _, testutils::Ledger, Address,
    BytesN, Env,
};

use crate::{InvoiceContract, InvoiceContractClient, InvoiceStatus};

#[contract]
pub struct MockRegistry;

#[contractimpl]
impl MockRegistry {
    pub fn is_verified(env: Env, address: Address) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey(address))
            .unwrap_or(false)
    }

    pub fn register(env: Env, address: Address) {
        env.storage()
            .persistent()
            .set(&DataKey(address.clone()), &true);
        env.storage()
            .persistent()
            .extend_ttl(&DataKey(address), 100, 2_000_000);
    }
}

#[contracttype]
pub struct DataKey(Address);

type Setup = (
    Env,
    InvoiceContractClient<'static>,
    Address,
    Address,
    MockRegistryClient<'static>,
);

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin, &registry_id);

    (env, client, issuer, buyer, registry_client)
}

#[test]
fn test_create_invoice_with_verified_parties() {
    let (env, client, issuer, buyer, _) = setup();
    let face_value: u128 = 1_000_000_000;
    let due_date = env.ledger().timestamp() + 86400;

    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date);
    let invoice = client.get(&invoice_id);

    assert_eq!(invoice.issuer, issuer);
    assert_eq!(invoice.buyer, buyer);
    assert_eq!(invoice.face_value, face_value);
    assert_eq!(invoice.due_date, due_date);
    assert_eq!(invoice.status, InvoiceStatus::Created);
    assert!(!invoice.issuer_confirmed);
    assert!(!invoice.buyer_confirmed);
}

#[test]
#[should_panic(expected = "InvoiceError::InvalidFaceValue")]
fn test_create_fails_zero_face_value() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    client.create(&issuer, &buyer, &0, &due_date);
}

#[test]
#[should_panic(expected = "InvoiceError::InvalidDueDate")]
fn test_create_fails_past_due_date() {
    let (env, client, issuer, buyer, _) = setup();
    let past_date = env.ledger().timestamp() - 1;
    client.create(&issuer, &buyer, &1_000_000_000, &past_date);
}

#[test]
fn test_list_for_financing() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);

    let result = client.list_for_financing(&invoice_id, &200);
    assert!(result);

    let invoice = client.get(&invoice_id);
    assert_eq!(invoice.status, InvoiceStatus::Listed);
    assert_eq!(invoice.discount_bps, 200);
}

#[test]
#[should_panic(expected = "InvoiceError::InvalidStatusTransition")]
fn test_list_fails_wrong_status() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.list_for_financing(&invoice_id, &200);
    client.list_for_financing(&invoice_id, &300);
}

#[test]
#[should_panic(expected = "InvoiceError::DiscountTooHigh")]
fn test_list_fails_discount_too_high() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.list_for_financing(&invoice_id, &5001);
}

#[test]
fn test_full_lifecycle() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Created);

    client.list_for_financing(&invoice_id, &200);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Listed);

    let pool = Address::generate(&env);
    client.set_pool_contract(&pool);

    let funded_amount: u128 = 980_000_000;
    let result = client.mark_funded(&invoice_id, &funded_amount);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Funded);

    client.mark_shipped(&invoice_id);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Active);

    client.confirm_delivery(&invoice_id, &issuer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Active);
    assert!(client.get(&invoice_id).issuer_confirmed);
    assert!(!client.get(&invoice_id).buyer_confirmed);

    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
    assert!(client.get(&invoice_id).issuer_confirmed);
    assert!(client.get(&invoice_id).buyer_confirmed);
}

#[test]
fn test_get_by_issuer_returns_correct_invoices() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;

    client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date);

    let invoices = client.get_by_issuer(&issuer);
    assert_eq!(invoices.len(), 2);

    let other = Address::generate(&env);
    let empty = client.get_by_issuer(&other);
    assert_eq!(empty.len(), 0);
}

#[test]
fn test_get_by_buyer_returns_correct_invoices() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;

    client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date);

    let invoices = client.get_by_buyer(&buyer);
    assert_eq!(invoices.len(), 2);
}

#[test]
fn test_get_by_status_returns_correct_invoices() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;

    client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date);

    let created = client.get_by_status(&InvoiceStatus::Created);
    assert_eq!(created.len(), 2);
}

#[test]
#[should_panic(expected = "InvoiceError::NotFound")]
fn test_get_unknown_panics() {
    let (env, client, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get(&fake_id);
}

#[test]
fn test_dual_confirmation_both_must_confirm() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.list_for_financing(&invoice_id, &200);

    let pool = Address::generate(&env);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &980_000_000);

    client.mark_shipped(&invoice_id);

    client.confirm_delivery(&invoice_id, &issuer);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Active);
    assert!(inv.issuer_confirmed);
    assert!(!inv.buyer_confirmed);

    client.confirm_delivery(&invoice_id, &buyer);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Confirmed);
    assert!(inv.issuer_confirmed);
    assert!(inv.buyer_confirmed);
}

#[test]
fn test_confirm_by_both_transitions_to_confirmed() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.list_for_financing(&invoice_id, &200);

    let pool = Address::generate(&env);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &980_000_000);
    client.mark_shipped(&invoice_id);

    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
#[should_panic(expected = "InvoiceError::NotAuthorized")]
fn test_confirm_delivery_wrong_party_panics() {
    let (env, client, issuer, _buyer, registry) = setup();
    let stranger = Address::generate(&env);
    // Register the buyer address we'll use for create
    let buyer = Address::generate(&env);
    registry.register(&buyer);

    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.list_for_financing(&invoice_id, &200);

    let pool = Address::generate(&env);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &980_000_000);
    client.mark_shipped(&invoice_id);

    client.confirm_delivery(&invoice_id, &stranger);
}

#[test]
fn test_trigger_default_requires_past_due_date() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.list_for_financing(&invoice_id, &200);

    let pool = Address::generate(&env);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &980_000_000);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    env.ledger().set_timestamp(due_date + 1);

    let result = client.trigger_default(&invoice_id);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Defaulted);
}

#[test]
fn test_get_by_status_filters_correctly() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;

    let id1 = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date);

    let created = client.get_by_status(&InvoiceStatus::Created);
    assert_eq!(created.len(), 2);

    client.list_for_financing(&id1, &200);
    let created = client.get_by_status(&InvoiceStatus::Created);
    assert_eq!(created.len(), 1);
    let listed = client.get_by_status(&InvoiceStatus::Listed);
    assert_eq!(listed.len(), 1);
}

#[test]
#[should_panic(expected = "InvoiceError::AlreadyConfirmed")]
fn test_double_confirmation_panics() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    client.list_for_financing(&invoice_id, &200);

    let pool = Address::generate(&env);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &980_000_000);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &issuer);
}

#[test]
fn test_status_transitions_full_lifecycle() {
    let (env, client, issuer, buyer, _) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Created);

    client.list_for_financing(&invoice_id, &200);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Listed);

    let pool = Address::generate(&env);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &980_000_000);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Funded);

    client.mark_shipped(&invoice_id);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Active);

    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);

    // Can't test repay without USDC asset
    // Can't test default without past due date
}
