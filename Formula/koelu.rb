class Koelu < Formula
  desc "Checks agent-made changes and reviews Dependabot pull requests"
  homepage "https://github.com/keys-i/koelu"
  url "https://github.com/keys-i/koelu.git",
      tag: "v0.6.10"
  version "0.6.10"
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
    ENV["KOELU_RUNS_DIR"] = (testpath/"runs").to_s
    assert_match '"runs": []', shell_output("#{bin}/koelu --output json runs")
    assert_match "Usage: koelu dependasolve", shell_output("#{bin}/koelu dependasolve --help")
  end
end
