set -e

RPI="prarthana@100.84.189.63"
REMOTE_DIR="~/nostreye-cam"
LOCAL_DIR="$(dirname "$0")/nostreye-cam"

echo "==> Checking RPi environment..."
ssh "$RPI" "
  source ~/.cargo/env || true
  echo '--- Rust ---'
  rustc --version 2>&1 || echo 'rustc not found'
  cargo --version 2>&1 || echo 'cargo not found'
  echo '--- libcamera ---'
  pkg-config --modversion libcamera 2>&1 || echo 'libcamera not found via pkg-config'
  libcamera-still --version 2>&1 | head -2 || echo 'libcamera-still not on PATH'
  echo '--- System ---'
  uname -m
  cat /etc/os-release | grep PRETTY_NAME
"

echo ""
echo "==> Creating project structure on RPi..."
ssh "$RPI" "mkdir -p $REMOTE_DIR/src"

echo ""
echo "==> Copying files to RPi..."
scp "$LOCAL_DIR/Cargo.toml"       "$RPI:$REMOTE_DIR/"
scp "$LOCAL_DIR/src/main.rs"      "$RPI:$REMOTE_DIR/src/"
scp "$LOCAL_DIR/src/camera.rs"    "$RPI:$REMOTE_DIR/src/"
scp "$LOCAL_DIR/src/signer.rs"    "$RPI:$REMOTE_DIR/src/"

echo ""
echo "==> Running cargo build on RPi (this may take a while)..."
ssh "$RPI" "source ~/.cargo/env || true; cd $REMOTE_DIR && cargo build 2>&1"

echo ""
echo "==> Build complete! Run the binary with:"
echo "    ssh $RPI 'source ~/.cargo/env || true; cd $REMOTE_DIR && cargo run'"
