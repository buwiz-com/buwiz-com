#!/bin/bash
# First boot of the Buwiz server: security updates, cloudflared, the AWS
# CLI, the users and the systemd units. The binaries, the Sender's unit and
# buwiz-env arrive later through SSM (install.sh).
set -euxo pipefail
export DEBIAN_FRONTEND=noninteractive

hostnamectl set-hostname buwiz-server
apt-get update
apt-get -y upgrade
apt-get install -y unattended-upgrades curl ca-certificates
# Security updates install themselves; reboot at 03:00 Manila time if needed.
cat > /etc/apt/apt.conf.d/52buwiz-upgrades <<'EOF'
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "19:00";
EOF
timedatectl set-timezone UTC

# cloudflared from Cloudflare's apt repository.
mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg -o /usr/share/keyrings/cloudflare-main.gpg
echo 'deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared any main' \
  > /etc/apt/sources.list.d/cloudflared.list
apt-get update
apt-get install -y cloudflared

snap install aws-cli --classic

useradd --system --no-create-home --shell /usr/sbin/nologin buwiz || true
# The Sender's own user: the app's user cannot read its settings or memory.
useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin buwiz-sender || true
install -d -m 0755 /opt/buwiz
install -d -m 0700 /etc/buwiz /etc/cloudflared
install -d -o buwiz-sender -g buwiz-sender -m 0700 /etc/buwiz-sender

# /usr/local/bin/buwiz-env, which writes the settings files from Parameter
# Store at each start, and the Sender's unit come with each release
# (install.sh).

cat > /etc/systemd/system/buwiz.service <<'EOF'
[Unit]
Description=Buwiz server
After=network-online.target
Wants=network-online.target

[Service]
# install.sh's drop-in (buwiz.service.d/env.conf) replaces this line with
# buwiz-env.service, and keeps the app from the metadata address.
ExecStartPre=+/usr/local/bin/buwiz-env
# Optional at load time: buwiz-env writes it before ExecStart reads it.
EnvironmentFile=-/etc/buwiz/env
ExecStart=/opt/buwiz/buwiz-cloud
User=buwiz
Restart=always
RestartSec=2
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectControlGroups=true
RestrictSUIDSGID=true
LockPersonality=true

[Install]
WantedBy=multi-user.target
EOF

# Tunnel credentials and config are written by the deploy step.
cat > /etc/systemd/system/cloudflared-buwiz.service <<'EOF'
[Unit]
Description=Cloudflare Tunnel for Buwiz
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/bin/cloudflared --no-autoupdate --config /etc/cloudflared/config.yml tunnel run
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
touch /var/lib/buwiz-bootstrap-done
