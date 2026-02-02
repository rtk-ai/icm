# typed: false
# frozen_string_literal: true

# Homebrew formula for icm
# To install: brew tap rtk-ai/tap && brew install icm
class Icm < Formula
  desc "Permanent memory for AI agents"
  homepage "https://github.com/rtk-ai/icm"
  version "0.0.1"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/rtk-ai/icm/releases/download/v#{version}/icm-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_INTEL"
    end

    on_arm do
      url "https://github.com/rtk-ai/icm/releases/download/v#{version}/icm-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/rtk-ai/icm/releases/download/v#{version}/icm-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_INTEL"
    end

    on_arm do
      url "https://github.com/rtk-ai/icm/releases/download/v#{version}/icm-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_ARM"
    end
  end

  def install
    bin.install "icm"
  end

  test do
    assert_match "icm #{version}", shell_output("#{bin}/icm --version")
  end
end
