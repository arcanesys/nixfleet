# Enrolling Agents

Each managed host runs an agent that connects to the control plane. The agent is a NixOS service module — enable it per host.

## Basic Setup

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://fleet.example.com:8443";
};
```

After rebuilding, the agent registers with the control plane and starts polling for work.

## Adding Tags

Tags group machines for targeted operations. Add them in the agent config:

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://fleet.example.com:8443";
  tags = [ "production" "web" "eu-west" ];
};
```

Tags can also be managed dynamically via the CLI:

```sh
nixfleet machines tag web-01 production eu-west
nixfleet machines untag web-01 eu-west
```

## Configuring Health Checks

Health checks run continuously on the agent and report to the control plane. They determine whether a deployment succeeded.

### Systemd checks

Verify critical services are running:

```nix
services.nixfleet-agent.healthChecks.systemd = [
  { units = [ "nginx.service" "postgresql.service" ]; }
];
```

### HTTP checks

Verify endpoints respond:

```nix
services.nixfleet-agent.healthChecks.http = [
  {
    url = "http://localhost:8080/health";
    interval = 5;
    timeout = 3;
    expectedStatus = 200;
  }
];
```

### Command checks

Run arbitrary health scripts:

```nix
services.nixfleet-agent.healthChecks.command = [
  {
    name = "disk-space";
    command = "test $(df --output=pcent / | tail -1 | tr -d ' %') -lt 90";
    interval = 10;
    timeout = 5;
  }
];
```

## Verify Enrollment

```sh
# From the CLI, list all registered machines
nixfleet machines list

# Filter by tag
nixfleet machines list --tag production
```

## Agent Options Reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | false | Enable the agent |
| `controlPlaneUrl` | str | — | Control plane URL (required) |
| `machineId` | str | hostname | Machine identifier |
| `pollInterval` | int | 300 | Seconds between polls |
| `tags` | list of str | `[]` | Machine tags |
| `healthInterval` | int | 60 | Seconds between health reports |
| `cacheUrl` | str or null | null | Binary cache URL |
| `dryRun` | bool | false | Check without applying |

## Next Steps

- [Deploying to Your Fleet](deploying.md) — rollouts and strategies
