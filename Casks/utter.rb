cask "utter" do
  version "0.3.0"
  sha256 "1c09c7a93e6b52cbf93d4d3a9baa09b8d49e6e2706974ba1459c0dd42cf1e754"

  url "https://github.com/jguice/utter/releases/download/v#{version}/utter-#{version}-macos-arm64.dmg"
  name "Utter"
  desc "Local, no-cloud push-to-talk voice dictation"
  homepage "https://github.com/jguice/utter"

  livecheck do
    url :stable
    strategy :github_latest
  end

  depends_on macos: ">= :ventura"

  app "utter.app"

  zap trash: [
    "~/Library/Application Support/utter",
    "~/Library/Preferences/com.utter.app.plist",
  ]

  caveats do
    <<~EOS
      Download the speech model (~650 MB, one-time):

        curl -fsSL https://raw.githubusercontent.com/jguice/utter/main/scripts/download-model.sh | bash

      utter requires these permissions (granted on first launch):

        - Microphone
        - Input Monitoring
        - Accessibility

      After granting, quit and relaunch utter.app from the menu bar.
    EOS
  end
end
