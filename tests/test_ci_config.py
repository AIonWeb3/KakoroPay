"""Guard that CI configuration stays present and complete."""

from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"


def test_ci_workflow_exists():
    assert WORKFLOW.is_file()


def test_ci_runs_rust_and_python_gates():
    text = WORKFLOW.read_text(encoding="utf-8")
    for needle in (
        "cargo fmt",
        "cargo clippy",
        "cargo test",
        "pytest",
        "ruff check",
    ):
        assert needle in text, f"CI workflow missing: {needle}"


def test_e2e_testnet_workflow_exists():
    e2e_workflow = ROOT / ".github" / "workflows" / "e2e-testnet.yml"
    assert e2e_workflow.is_file()
    text = e2e_workflow.read_text(encoding="utf-8")
    assert "cron:" in text
    assert "workflow_dispatch:" in text
    assert "upload-artifact" in text

