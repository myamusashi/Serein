cask "serein" do
  version "1.0.0-nightly.20260914.17"
  sha256 "054896e0a2d11d7098086bab73a7d3188213c0210a20043c2abc2602074c8b6f"

  url "https://github.com/ViceVerse-cz/Serein/releases/download/v#{version}/serein-v#{version}-macOS-ARM64.zip"
  name "Serein"
  desc "Experimental native Discord client"
  homepage "https://github.com/ViceVerse-cz/Serein"

  depends_on arch: :arm64
  depends_on macos: :sonoma

  app "Serein.app"
end
