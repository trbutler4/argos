#!/usr/bin/env python3
"""Exercise the real CLI's configuration contract, without contacting other hosts."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ARGOS_BINARY", ROOT / "target/debug/argos"))


def main():
    with tempfile.TemporaryDirectory(prefix="argos-config-", dir=os.environ.get("XDG_RUNTIME_DIR")) as temp:
        root = Path(temp)
        env = dict(os.environ, XDG_CONFIG_HOME=str(root / "xdg"), HOME=str(root / "home"))
        config = root / "inventory.toml"
        sock = root / "missing.sock"
        valid = f'schema_version=1\n[client]\nmachine_id="here"\n[machines.here]\nsocket={json.dumps(str(sock))}\n'
        config.write_text(valid)

        def cli(*args, code=0):
            r = subprocess.run([str(BINARY), "list", *args], env=env, capture_output=True, text=True, timeout=10)
            assert r.returncode == code, (args, r.returncode, r.stdout, r.stderr)
            return r

        cli("--host", "here", code=2)  # Never silently fall back from a host filter.
        cli("--config", str(root / "absent"), code=2)
        for arguments in [("--local", "--config", str(config)), ("--socket", str(sock), "--host", "here")]:
            cli(*arguments, code=2)
        expected = json.loads(cli("--config", str(config), "--json").stdout)["hosts"]
        assert expected == [{"id": "here", "status": "ok", "sessions": [], "error": None}]
        assert json.loads(cli("--config", str(config), "--host", "here", "--json").stdout)["hosts"] == expected
        cli("--config", str(config), "--host", "missing", code=2)
        implicit = Path(env["XDG_CONFIG_HOME"]) / "argos/config.toml"
        implicit.parent.mkdir(parents=True)
        implicit.write_text(valid)
        assert json.loads(cli("--json").stdout)["hosts"] == expected
        implicit.write_text("broken = [")
        cli("--json", code=2)
        assert json.loads(cli("--socket", str(sock), "--json").stdout)["hosts"][0]["status"] == "ok"
        assert json.loads(cli("--local", "--json").stdout)["hosts"][0]["status"] == "ok"
        vm_config = valid + '[vm.guest_tmux]\nconfig_text="set -g mouse on"\n'
        config.write_text(vm_config)
        assert json.loads(cli("--config", str(config), "--json").stdout)["hosts"] == expected

        for bad in [valid.replace("schema_version=1", "schema_version=2"),
                    valid.replace("[client]", "unknown=1\n[client]"),
                    valid.replace('[client]', '[client]\nconnect_timeout_seconds=0'),
                    valid.replace('[client]', '[client]\nmax_parallel_probes=0'),
                    valid.replace('machine_id="here"', 'machine_id="missing"'),
                    valid + '[machines.peer]\nssh_alias="-oProxyCommand=bad"\n',
                    valid + '[machines.peer]\nssh_alias="host; touch never"\n',
                    valid + '[machines.peer]\n',
                    valid + '[machines.peer]\nssh_alias="valid"\nsocket=""\n',
                    valid + '[vm.guest_tmux]\nconfig_text="a"\nconfig_path="b"\n']:
            config.write_text(bad)
            cli("--config", str(config), "--json", code=2)
        assert not sock.exists(), "Config tests started a server"
    print("PASS: real CLI config loading, defaults, selection, invalid schemas, aliases, limits and local bypass.")


if __name__ == "__main__":
    main()
