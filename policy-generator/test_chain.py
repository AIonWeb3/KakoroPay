from chain import events_to_tx_log, fetch_event_pages, generate_policy_from_chain


def test_events_become_policy_spec():
    payload = {
        "account_id": "GAGENT",
        "events": [
            {
                "contract_id": "CWEATHER",
                "topics": ["get_weather"],
                "amount": 50,
                "timestamp": "2026-01-01T00:00:00Z",
            },
            {
                "contract_id": "CWEATHER",
                "topics": ["get_weather"],
                "amount": 50,
                "timestamp": "2026-01-01T01:00:00Z",
            },
        ],
    }
    log = events_to_tx_log(payload)
    assert log[0]["method"] == "get_weather"
    assert log[0]["agent_pubkey"] == "GAGENT"
    policy = generate_policy_from_chain(payload)
    assert policy.allowed_contracts[0].contract_id == "CWEATHER"
    assert policy.agent_signer == "GAGENT"


def test_pagination_stops_when_cursor_ends():
    pages = {
        None: {"events": [{"contract_id": "C1", "method": "a", "amount": 1}], "cursor": "n"},
        "n": {"events": [{"contract_id": "C1", "method": "a", "amount": 2}], "cursor": None},
    }

    def getter(_account, cursor):
        return pages[cursor]

    merged = fetch_event_pages("G1", getter)
    assert len(merged["events"]) == 2
