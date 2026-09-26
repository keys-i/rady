class Pekin < Formula
  desc "Checks agent-made changes and reviews Dependabot pull requests"
  homepage "https://github.com/keys-i/rady"
  url "https://github.com/keys-i/rady.git",
      tag: "v0.6.8"
  version "0.6.8"
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
    ENV["PEKIN_RUNS_DIR"] = (testpath/"runs").to_s
    assert_match '"runs": []', shell_output("#{bin}/pekin --output json runs")
    assert_match "Usage: pekin dependasolve", shell_output("#{bin}/pekin dependasolve --help")
  end
end
