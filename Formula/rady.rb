class Rady < Formula
  desc "Checks agent-made changes and reviews Dependabot pull requests"
  homepage "https://github.com/keys-i/rady"
  url "https://github.com/keys-i/rady.git",
      tag: "v0.5.6"
  version "0.5.6"
  license "MIT"

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
    assert_match "Usage: rady dependasolve", shell_output("#{bin}/dependasolver --help")
  end
end
