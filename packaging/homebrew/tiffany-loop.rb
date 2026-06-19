class TiffanyLoop < Formula
  desc "Lightweight multi-agent orchestration shell for LLM CLIs"
  homepage "https://github.com/macguffinQ/Tiffany"

  # Template only. The release workflow writes the published tap formula with
  # the current tag, archive URL, and checksums.
  url "https://github.com/macguffinQ/Tiffany/archive/refs/tags/v0.1.11.tar.gz"
  sha256 "REPLACE_WITH_RELEASE_SOURCE_SHA256"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args, "--profile", "tiffany-dist"
    system "cargo", "install", *std_cargo_args(path: "tiffany-ui/codex-rs/tiffany-cli"), "--profile", "tiffany-dist"

    if OS.mac? || OS.linux?
      system "strip", "#{bin}/orchestrator", "#{bin}/tiffany"
    end
  end

  test do
    assert_match "orchestrator", shell_output("#{bin}/orchestrator --help")
    assert_match "orchestrator", shell_output("#{bin}/tiffany orchestrator --help 2>/dev/null")
  end
end
