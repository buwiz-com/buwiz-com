#!/bin/bash
# Runs on the server through SSM (deploy.sh): installs release $1 from the
# deploy bucket $2 (region $3) and (re)starts the app and the tunnel.
set -euo pipefail
release=$1
bucket=$2
export AWS_REGION=${3:-ap-southeast-1}
aws=/snap/bin/aws

$aws s3 cp "s3://$bucket/releases/$release/buwiz-cloud" /opt/buwiz/buwiz-cloud.new --no-progress
chmod 0755 /opt/buwiz/buwiz-cloud.new
mv /opt/buwiz/buwiz-cloud.new /opt/buwiz/buwiz-cloud
$aws s3 cp "s3://$bucket/releases/$release/buwiz-sender" /opt/buwiz/buwiz-sender.new --no-progress
chmod 0755 /opt/buwiz/buwiz-sender.new
mv /opt/buwiz/buwiz-sender.new /opt/buwiz/buwiz-sender
echo "$release" > /opt/buwiz/RELEASE

# The Sender: loopback only (SENDER_HOST defaults to 127.0.0.1:3009); the
# app reaches it at SENDER_URL. Same settings file as the app.
cat > /etc/systemd/system/buwiz-sender.service <<'UNIT'
[Unit]
Description=Buwiz Sender
After=network-online.target
Wants=network-online.target

[Service]
ExecStartPre=+/usr/local/bin/buwiz-env
EnvironmentFile=-/etc/buwiz/env
ExecStart=/opt/buwiz/buwiz-sender
User=buwiz
Restart=always
RestartSec=2
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true

[Install]
WantedBy=multi-user.target
UNIT
mkdir -p /etc/systemd/system/buwiz.service.d
cat > /etc/systemd/system/buwiz.service.d/sender.conf <<'UNIT'
[Unit]
After=buwiz-sender.service
Wants=buwiz-sender.service
UNIT
systemctl daemon-reload

# OCR models (OCR_MODEL_DIR=/opt/buwiz/ocr-models comes from /buwiz/*).
mkdir -p /opt/buwiz/ocr-models
for model in pp-ocrv5_mobile_det.onnx latin_pp-ocrv5_mobile_rec.onnx ppocrv5_latin_dict.txt; do
  $aws s3 cp "s3://$bucket/releases/$release/ocr-models/$model" "/opt/buwiz/ocr-models/$model" --no-progress
done
chmod 0644 /opt/buwiz/ocr-models/*

# OCR holds about 350 MB once warm; a swap file keeps a burst from killing
# the app on this 1 GB host.
if ! swapon --show=NAME --noheadings | grep -q /swapfile; then
  test -f /swapfile || { fallocate -l 1G /swapfile && chmod 0600 /swapfile && mkswap /swapfile >/dev/null; }
  swapon /swapfile
  grep -q '^/swapfile ' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

# Tunnel settings: every hostname routed to this tunnel reaches the app.
umask 077
tunnel_id=$($aws ssm get-parameter --name /buwiz/tunnel/id --query Parameter.Value --output text)
# The snap aws writes nothing when stdout is a file, so pipe it.
$aws ssm get-parameter --name /buwiz/tunnel/credentials --with-decryption --query Parameter.Value --output text \
  | cat > /etc/cloudflared/credentials.json
test -s /etc/cloudflared/credentials.json
cat > /etc/cloudflared/config.yml <<EOF
tunnel: $tunnel_id
credentials-file: /etc/cloudflared/credentials.json
ingress:
  - service: http://127.0.0.1:8080
EOF

systemctl enable buwiz-sender.service buwiz.service cloudflared-buwiz.service
systemctl restart buwiz-sender.service
systemctl restart buwiz.service
systemctl restart cloudflared-buwiz.service
# Start-up can take a while (the BIR sign loads for up to 10 s before the
# port opens): wait up to 45 s for the app; the check below still fails if
# it never answers.
for _ in $(seq 45); do
  curl -fs -o /dev/null http://127.0.0.1:8080/ && break
  sleep 1
done
systemctl is-active buwiz-sender.service buwiz.service cloudflared-buwiz.service || true
journalctl -u buwiz-sender.service -n 5 --no-pager -o cat
curl -fsS -o /dev/null -w 'local app: HTTP %{http_code}\n' http://127.0.0.1:8080/
journalctl -u buwiz.service -n 5 --no-pager -o cat
