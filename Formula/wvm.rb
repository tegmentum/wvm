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
  version "0.4.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-aarch64-macos"
      sha256 "94c63fa25e1c78545a03f621da6143e0a6560f4500d8fe9f66aa63ef51557312"
    end
    on_intel do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-x86_64-macos"
      sha256 "8dc240882bed65c566e3878dbfea8bf2e7a2f3052367ebf7b34529c81990d0c1"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-aarch64-linux"
      sha256 "109b177ec18c4b9d279656d3d7e3729ca3016815c2792051929bc5baaa1d0708"
    end
    on_intel do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-x86_64-linux"
      sha256 "f026016d8c7afe5a2897a9fd2e2be89325c0118d93be2d99b9d7ea626be63553"
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
