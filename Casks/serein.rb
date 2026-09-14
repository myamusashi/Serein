cask "serein" do
  version "1.0.0-nightly.20260914.19"
  sha256 "a164bae53249f309743be53ae4cadd70a9458d4f816f0baba6d06bcce5bf7dbc"

  url "https://github.com/ViceVerse-cz/Serein/releases/download/v#{version}/serein-v#{version}-macOS-ARM64.zip"
  name "Serein"
  desc "Experimental native Discord client"
  homepage "https://github.com/ViceVerse-cz/Serein"

  depends_on arch: :arm64
  depends_on macos: :sonoma

  app "Serein.app"
end
