git clone https://github.com/XaviCodeDev1000/webfang.git
cd webfang
cmake --version        # must print a version; install cmake if empty
cargo build --release --locked --features mcp -p webfang_cli
./target/release/webfang --version
