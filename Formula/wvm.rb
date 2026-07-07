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
  desc "Wasmtime Version Manager — installs and manages Wasmtime runtimes"
  homepage "https://github.com/tegmentum/wvm"
  version "0.5.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-aarch64-macos"
      sha256 "a06a36e08382db48e8db0e4a15327638257e508f7583e7ae7e445929da6f4533"
    end
    on_intel do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-x86_64-macos"
      sha256 "4c34d0db8f5285bbf46848ea06114802351bb57126edb9c3c2671ad9d62cb183"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-aarch64-linux"
      sha256 "f943b3bd5e2f20f4ff156b02bb53f46bd23b07445b8b6cfe020b0dac9b8651ab"
    end
    on_intel do
      url "https://github.com/tegmentum/wvm/releases/download/v#{version}/wvm-x86_64-linux"
      sha256 "a6cda4c4ec23eebf8f2375b1138c0a47a85457b5ef436544fd7dd9887c604ff3"
    end
  end

  def install
    # The release asset is a bare binary named wvm-<arch>-<os>. GitHub's
    # release download loses the exec bit, so restore it before running the
    # binary to generate completions below.
    bin.install Dir["wvm-*"].first => "wvm"
    chmod 0755, bin/"wvm"
    # Emit and install completion scripts. `wvm completions <shell>` prints
    # the script to stdout — Homebrew's helper takes care of putting each
    # generated file in the right per-shell location.
    generate_completions_from_executable(bin/"wvm", "completions")
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
