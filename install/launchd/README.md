# Launchd Service for Post-Cortex Daemon (macOS)

This launchd configuration allows you to run post-cortex-daemon as a background service that starts automatically on macOS.

## Installation

### 1. Install the binary

```bash
# Option A: Via Homebrew (recommended)
brew install julymetodiev/tap/post-cortex

# Option B: Direct download
# Download post-cortex-daemon-aarch64-apple-darwin (Apple Silicon) or
#          post-cortex-daemon-x86_64-apple-darwin (Intel Mac) from releases
chmod +x post-cortex-daemon-*
sudo mv post-cortex-daemon-* /usr/local/bin/post-cortex-daemon
```

### 2. Copy launchd plist file

```bash
# Create LaunchAgents directory if it doesn't exist
mkdir -p ~/Library/LaunchAgents

# Copy plist file
cp install/launchd/com.post-cortex.daemon.plist ~/Library/LaunchAgents/

# Adjust permissions
chmod 644 ~/Library/LaunchAgents/com.post-cortex.daemon.plist
```

### 3. Initialize configuration (optional)

```bash
post-cortex-daemon init
```

This creates `~/.post-cortex/daemon.toml` with default settings.

## Usage

### Load and start the daemon

```bash
launchctl load ~/Library/LaunchAgents/com.post-cortex.daemon.plist
```

This will:
- Start the daemon immediately
- Configure it to auto-start on login

### Check if daemon is running

```bash
# Check launchctl
launchctl list | grep post-cortex

# Or use the daemon's status command
post-cortex-daemon status

# Or check the process
ps aux | grep post-cortex-daemon
```

### View logs

```bash
# Standard output
tail -f /tmp/post-cortex.log

# Error output
tail -f /tmp/post-cortex-error.log

# Or use Console.app and search for "post-cortex"
```

### Stop the daemon

```bash
launchctl stop com.post-cortex.daemon
```

Note: This only stops it temporarily. It will restart on next login or system reboot.

### Unload (stop and prevent auto-start)

```bash
launchctl unload ~/Library/LaunchAgents/com.post-cortex.daemon.plist
```

### Restart the daemon

```bash
launchctl kickstart -k gui/$(id -u)/com.post-cortex.daemon
```

Or:

```bash
launchctl unload ~/Library/LaunchAgents/com.post-cortex.daemon.plist
launchctl load ~/Library/LaunchAgents/com.post-cortex.daemon.plist
```

## Configuration

The plist file uses these environment variables:
- `RUST_LOG=info` - Logging level (change to `debug` for verbose logs)
- `PC_HOST=127.0.0.1` - Bind address (localhost only by default)
- `PC_PORT=3737` - Port number

Binding to a non-loopback address? Also set `PC_ALLOWED_HOSTS` (comma-separated
host names/IPs clients use, e.g. `my-mac.tailnet.ts.net,100.64.0.7`) or remote
clients get 403, and `PC_AUTH_TOKEN`. See [docs/DAEMON_MODE.md](../../docs/DAEMON_MODE.md#allowed-hosts-dns-rebinding).

### Customizing via plist file

Edit `~/Library/LaunchAgents/com.post-cortex.daemon.plist` and reload:

```bash
launchctl unload ~/Library/LaunchAgents/com.post-cortex.daemon.plist
launchctl load ~/Library/LaunchAgents/com.post-cortex.daemon.plist
```

### Customizing via config file

Create `~/.post-cortex/daemon.toml`:

```toml
host = "127.0.0.1"
port = 3737
data_dir = "~/.post-cortex/data"
```

Priority: Environment variables > Config file > Defaults

## Troubleshooting

### Service won't start

```bash
# Check launchctl status
launchctl list | grep post-cortex

# Check logs
cat /tmp/post-cortex-error.log

# Try running manually
/usr/local/bin/post-cortex-daemon start
```

### Port already in use

```bash
# Check what's using port 3737
lsof -i :3737

# Kill the process
kill $(lsof -t -i:3737)

# Or change port in config/environment
```

### Permission issues

```bash
# Fix plist permissions
chmod 644 ~/Library/LaunchAgents/com.post-cortex.daemon.plist

# Fix binary permissions
sudo chmod +x /usr/local/bin/post-cortex-daemon

# Ensure data directory exists
mkdir -p ~/.post-cortex/data
```

### Daemon keeps restarting

```bash
# Check error logs
cat /tmp/post-cortex-error.log

# Increase throttle interval in plist (edit ThrottleInterval)

# Temporarily disable auto-restart (edit KeepAlive to false)
```

### Changes to plist not taking effect

```bash
# Always reload after editing plist
launchctl unload ~/Library/LaunchAgents/com.post-cortex.daemon.plist
launchctl load ~/Library/LaunchAgents/com.post-cortex.daemon.plist
```

## Advanced Usage

### Disable auto-start on login

Edit the plist and set:
```xml
<key>RunAtLoad</key>
<false/>
```

Then reload:
```bash
launchctl unload ~/Library/LaunchAgents/com.post-cortex.daemon.plist
launchctl load ~/Library/LaunchAgents/com.post-cortex.daemon.plist
```

### Change log location

Edit the plist file and modify:
```xml
<key>StandardOutPath</key>
<string>/path/to/your/post-cortex.log</string>
<key>StandardErrorPath</key>
<string>/path/to/your/post-cortex-error.log</string>
```

### Debug mode

Edit the plist and change:
```xml
<key>RUST_LOG</key>
<string>debug</string>
```

## Uninstall

```bash
# Unload and stop the daemon
launchctl unload ~/Library/LaunchAgents/com.post-cortex.daemon.plist

# Remove plist file
rm ~/Library/LaunchAgents/com.post-cortex.daemon.plist

# Remove logs
rm /tmp/post-cortex.log /tmp/post-cortex-error.log

# Remove data (optional)
rm -rf ~/.post-cortex

# Remove binary (if not using Homebrew)
sudo rm /usr/local/bin/post-cortex-daemon
```

If installed via Homebrew:
```bash
brew uninstall post-cortex
```

## System Service (Advanced)

To run as a system-wide service (not recommended for most users):

1. Copy plist to `/Library/LaunchDaemons/` instead of `~/Library/LaunchAgents/`
2. Use `sudo launchctl load /Library/LaunchDaemons/com.post-cortex.daemon.plist`
3. Update `ProgramArguments` paths to use absolute paths

Note: System services run as root and start at boot, before any user logs in.
