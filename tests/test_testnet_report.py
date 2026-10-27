import json
import subprocess
import sys
from pathlib import Path

from testnet_report import build_report, deterministic_tx_hash

ROOT = Path(__file__).resolve().parent.parent


def test_dry_run_hash_is_stable():
    report = build_report(network="testnet", mode="dry-run")
    assert report["tx_hash"] == deterministic_tx_hash()
    assert len(report["tx_hash"]) == 64


def test_deploy_script_wires_dry_run_and_replace():
    text = (ROOT / "scripts" / "deploy_testnet.sh").read_text(encoding="utf-8")
    assert "AGENTPAY_DRY_RUN" in text
    assert "replace_policy" in text
    assert "testnet_report.py" in text


def test_report_cli_writes_hash(tmp_path):
    out = tmp_path / "deploy_report.json"
    subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts" / "testnet_report.py"),
            "--dry-run",
            "--out",
            str(out),
        ],
        check=True,
    )
    body = json.loads(out.read_text(encoding="utf-8"))
    assert body["tx_hash"] == deterministic_tx_hash()
