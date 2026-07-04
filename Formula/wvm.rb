# Homebrew formula for wvm.
#
#   brew tap tegmentum/wvm https://github.com/tegmentum/wvm
#   brew install wvm
#
# This installs the single native `wvm` binary (the WASM app is embedded). On
# first use, wvm downloads and locks a protected seed Wasmtime runtime.
#
# Releasing: build the per-platform binaries (see RELEASING in the README),
# upload them to the GitHub release as `wvm-<arch>-<os>`, then bump `version`
# and replace each `sha256` below (`shasum -a 256 wvm-<arch>-<os>`).
class Wvm < Formula
  desc "WebAssembly Version Manager — installs and manages Wasmtime runtimes"
  homepage "https://github.com/tegmentum/wvm"
  version "0.2.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-aarch64-macos"
      sha256 "560b2de17f4c7528f99a1f22517e65aadadd12beee5e973926683b335df35b60"
    end
    on_intel do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-x86_64-macos"
      sha256 "4eec3fc8efa8247923e326e1eb40d42ca9ad00223318c7db25d69f8e807e9218"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-aarch64-linux"
      sha256 "f8d84a56bd88e045d9b635732fcc344ccad53d93546a1bcd9145f5f5eda10163"
    end
    on_intel do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-x86_64-linux"
      sha256 "283aedea4be8ca7059068fbd05b1e140d3aa84bc7ba504a9e356d7b986bcf3c1"
    end
  end

  def install
    # The release asset is a bare binary named wvm-<arch>-<os>.
    bin.install Dir["wvm-*"].first => "wvm"
  end

  def caveats
    <<~EOS
      On first use, wvm downloads and locks a protected seed Wasmtime runtime,
      then runs as a WebAssembly component on it. Get started with:
        wvm install latest && wvm default latest
    EOS
  end

  test do
    assert_match "wvm #{version}", shell_output("#{bin}/wvm --version")
  end
end
