class Rady < Formula
  desc "Checks agent-made changes and reviews Dependabot pull requests"
  homepage "https://github.com/keys-i/rady"
  url "https://github.com/keys-i/rady.git",
      tag: "v0.6.7"
  version "0.6.7"
  license "MIT"

  deprecate! date: "2026-09-26", because: "was renamed to Pekin", replacement_formula: "pekin"

  depends_on "rust" => :build
  deny_network_access!

  def fetch
    system "cargo", "fetch", "--locked", "--target", "host-tuple"
  end

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    ENV["RADY_RUNS_DIR"] = (testpath/"runs").to_s
    assert_match '"runs": []', shell_output("#{bin}/rady --output json runs")
    assert_match "Usage: rady dependasolve", shell_output("#{bin}/rady dependasolve --help")
  end
end
