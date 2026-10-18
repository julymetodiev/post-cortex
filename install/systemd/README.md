# Systemd Service for Post-Cortex Daemon (Linux)

This systemd service allows you to run post-cortex-daemon as a user service that starts automatically.

## Installation

### 1. Install the binary

```bash
# Option A: Via Homebrew (recommended)
brew install julymetodiev/tap/post-cortex

# Option B: Direct download
# Download post-cortex-daemon-x86_64-unknown-linux-gnu from releases
chmod +x post-cortex-daemon-x86_64-unknown-linux-gnu
sudo mv post-cortex-daemon-x86_64-unknown-linux-gnu /usr/local/bin/post-cortex-daemon
```

### 2. Copy systemd service file

```bash
# Create systemd user directory if it doesn't exist
mkdir -p ~/.config/systemd/user

# Copy service file
cp install/systemd/post-cortex.service ~/.config/systemd/user/

# Reload systemd
systemctl --user daemon-reload
```

### 3. Initialize configuration (optional)

```bash
post-cortex-daemon init
```

This creates `~/.post-cortex/daemon.toml` with default settings.

## Usage

### Start the daemon

```bash
systemctl --user start post-cortex
```

### Enable auto-start on login

```bash
systemctl --user enable post-cortex
```

### Check status

```bash
systemctl --user status post-cortex
```

### View logs

```bash
# Live logs
journalctl --user -u post-cortex -f

# Last 100 lines
journalctl --user -u post-cortex -n 100

# Since boot
journalctl --user -u post-cortex -b
```

### Stop the daemon

```bash
systemctl --user stop post-cortex
```

### Restart the daemon

```bash
systemctl --user restart post-cortex
```

### Disable auto-start

```bash
systemctl --user disable post-cortex
```

## Configuration

The service file uses these environment variables:
- `RUST_LOG=info` - Logging level (change to `debug` for verbose logs)
- `PC_HOST=127.0.0.1` - Bind address (localhost only by default)
- `PC_PORT=3737` - Port number

Binding to a non-loopback address? Also set `PC_ALLOWED_HOSTS` (comma-separated
host names/IPs clients use, e.g. `my-mac.tailnet.ts.net,100.64.0.7`) or remote
clients get 403, and `PC_AUTH_TOKEN`. See [docs/DAEMON_MODE.md](../../docs/DAEMON_MODE.md#allowed-hosts-dns-rebinding).

To customize, edit `~/.config/systemd/user/post-cortex.service` and reload:

```bash
systemctl --user daemon-reload
systemctl --user restart post-cortex
```

Or create a config file at `~/.post-cortex/daemon.toml`:

```toml
host = "127.0.0.1"
port = 3737
data_dir = "~/.post-cortex/data"
```

## Security

The service includes security hardening:
- `NoNewPrivileges=true` - Prevents privilege escalation
- `PrivateTmp=true` - Isolated /tmp directory
- `ProtectSystem=strict` - Read-only system directories
- `ProtectHome=read-only` - Read-only home directory (except ~/.post-cortex)
- `ReadWritePaths=%h/.post-cortex` - Only ~/.post-cortex is writable

## Troubleshooting

### Service won't start

```bash
# Check status for errors
systemctl --user status post-cortex

# Check logs
journalctl --user -u post-cortex -n 50
```

### Port already in use

```bash
# Check what's using port 3737
lsof -i :3737

# Change port in config or environment
```

### Permission issues

```bash
# Ensure ~/.post-cortex exists and is writable
mkdir -p ~/.post-cortex/data
chmod 755 ~/.post-cortex
```

## Uninstall

```bash
# Stop and disable service
systemctl --user stop post-cortex
systemctl --user disable post-cortex

# Remove service file
rm ~/.config/systemd/user/post-cortex.service

# Reload systemd
systemctl --user daemon-reload

# Remove data (optional)
rm -rf ~/.post-cortex
```
