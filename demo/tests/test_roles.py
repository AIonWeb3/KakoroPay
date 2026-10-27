from auth import OPERATOR_ENV, READER_ENV
from httputil import asgi_request
from server import app


def test_reader_can_discover_and_budget(monkeypatch):
    monkeypatch.setenv(READER_ENV, "read-key")
    monkeypatch.setenv(OPERATOR_ENV, "op-key")
    headers = {"X-Api-Key": "read-key"}
    assert (
        asgi_request(
            app, "POST", "/api/discover", json={"query": "weather"}, headers=headers
        ).status_code
        == 200
    )
    assert asgi_request(app, "GET", "/api/budget", headers=headers).status_code == 200


def test_reader_cannot_pay(monkeypatch):
    monkeypatch.setenv(READER_ENV, "read-key")
    monkeypatch.setenv(OPERATOR_ENV, "op-key")
    res = asgi_request(
        app,
        "POST",
        "/api/pay",
        json={"resource_id": "weather-oracle"},
        headers={"X-Api-Key": "read-key"},
    )
    assert res.status_code == 401


def test_operator_can_pay(monkeypatch):
    monkeypatch.setenv(OPERATOR_ENV, "op-key")
    res = asgi_request(
        app,
        "POST",
        "/api/pay",
        json={"resource_id": "weather-oracle"},
        headers={"X-Api-Key": "op-key"},
    )
    assert res.status_code == 200
