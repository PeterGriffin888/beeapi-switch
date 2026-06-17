cask "beeapi-switch" do
  version "0.1.4"

  if Hardware::CPU.arm?
    url "https://github.com/PeterGriffin888/beeapi-switch/releases/download/v#{version}/BeeAPI.Switch_#{version}_aarch64.dmg"
    sha256 :no_check
  else
    url "https://github.com/PeterGriffin888/beeapi-switch/releases/download/v#{version}/BeeAPI.Switch_#{version}_x64.dmg"
    sha256 :no_check
  end

  name "BeeAPI Switch"
  desc "One-click configuration tool for AI coding CLIs (Claude Code, Codex, Gemini CLI, OpenCode, OpenClaw)"
  homepage "https://github.com/PeterGriffin888/beeapi-switch"

  app "BeeAPI Switch.app"

  zap trash: [
    "~/.beeapi-switch",
  ]
end
