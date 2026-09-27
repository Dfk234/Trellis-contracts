
use crate::AidContract;
use soroban_sdk::{Address, Env};

#[test]
fn test_list_aids() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, AidContract);
    let client = AidContractClient::new(&env, &contract_id);

    let admin = Address::random(&env);
    client.initialize(&admin);

    let donor1 = Address::random(&env);
    let donor2 = Address::random(&env);
    let recipient = Address::random(&env);
    let token = Address::random(&env);

    client.create_aid(&donor1, &recipient, &token, &100, &(env.ledger().sequence() + 100));
    client.create_aid(&donor2, &recipient, &token, &200, &(env.ledger().sequence() + 100));
    client.create_aid(&donor1, &recipient, &token, &300, &(env.ledger().sequence() + 100));

    let response = client.list_aids(&2, &None);
    assert_eq!(response.aids.len(), 2);
    assert_eq!(response.next_cursor, Some(2));

    let response = client.list_aids(&2, &Some(2));
    assert_eq!(response.aids.len(), 1);
    assert_eq!(response.next_cursor, None);
}

#[test]
fn test_list_aids_by_donor() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, AidContract);
    let client = AidContractClient::new(&env, &contract_id);

    let admin = Address::random(&env);
    client.initialize(&admin);

    let donor1 = Address::random(&env);
    let donor2 = Address::random(&env);
    let recipient = Address::random(&env);
    let token = Address::random(&env);

    client.create_aid(&donor1, &recipient, &token, &100, &(env.ledger().sequence() + 100));
    client.create_aid(&donor2, &recipient, &token, &200, &(env.ledger().sequence() + 100));
    client.create_aid(&donor1, &recipient, &token, &300, &(env.ledger().sequence() + 100));

    let response = client.list_aids_by_donor(&donor1, &1, &None);
    assert_eq!(response.aids.len(), 1);
    assert_eq!(response.next_cursor, Some(1));

    let response = client.list_aids_by_donor(&donor1, &1, &Some(1));
    assert_eq!(response.aids.len(), 1);
    assert_eq!(response.next_cursor, Some(3));
}