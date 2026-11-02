#!/bin/bash
# First boot of the Buwiz server: security updates, cloudflared, the AWS
# CLI and the systemd units. The app binary arrives later through SSM.
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
install -d -m 0755 /opt/buwiz
install -d -m 0700 /etc/buwiz /etc/cloudflared

# Writes the app's environment from Parameter Store (/buwiz/*, not nested).
cat > /usr/local/bin/buwiz-env <<'EOF'
#!/bin/bash
set -euo pipefail
umask 077
aws ssm get-parameters-by-path --region ap-southeast-1 --path /buwiz --with-decryption \
  --query 'Parameters[].[Name,Value]' --output text \
  | sed -E 's#^/buwiz/([A-Z0-9_]+)\t#\1=#' > /etc/buwiz/env.new
mv /etc/buwiz/env.new /etc/buwiz/env
EOF
chmod 0755 /usr/local/bin/buwiz-env

cat > /etc/systemd/system/buwiz.service <<'EOF'
[Unit]
Description=Buwiz server
After=network-online.target
Wants=network-online.target

[Service]
ExecStartPre=+/usr/local/bin/buwiz-env
# Optional at load time: ExecStartPre writes it before ExecStart reads it.
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
