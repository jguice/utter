class Utter < Formula
  desc "Local, no-cloud push-to-talk voice dictation"
  homepage "https://github.com/jguice/utter"
  license "MIT"

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/jguice/utter/releases/download/v0.3.0/utter-0.3.0-linux-arm64.tar.gz"
      sha256 "6456172b0d3990d7a7894fd1be5dea97d1a3e6bd9173c1d7f4da36bb5f06a992"
    else
      url "https://github.com/jguice/utter/releases/download/v0.3.0/utter-0.3.0-linux-amd64.tar.gz"
      sha256 "d27e8739ece29f1d65a24b5b5bcf4bb6b3926e782082653115d312dcbace3c42"
    end
  end

  livecheck do
    url :stable
    strategy :github_latest
  end

  def install
    bin.install "utter"

    # Patch and install systemd user units.
    # The project ships two coordinated services (utter-daemon + utter-watcher)
    # that need After=graphical-session.target and PartOf=graphical-session.target.
    # Homebrew's auto-generated service stanza can't express this, so we install
    # the project's own units with the binary path corrected.
    %w[utter-daemon.service utter-watcher.service].each do |svc|
      content = File.read(buildpath / "packaging/systemd/#{svc}")
      content.gsub!("/usr/bin/utter", "#{opt_bin}/utter")
      (share/"systemd/user").write_bytes "#{svc}", content
    end

    # Udev rule: grants the active user read access to keyboard evdev devices
    # via the uaccess tag (no `input` group membership required).
    (share/"udev/rules.d").install buildpath / "packaging/udev/90-utter.rules"

    # Convenience: install the model-download script.
    bin.install buildpath / "scripts/download-model.sh"

    # Docs
    (share/"doc/utter").install %w[LICENSE NOTICE README.md BACKLOG.md]
  end

  def caveats
    <<~EOS
      First-time setup:

        1. Install and reload udev rules (for keyboard access):
           sudo cp $(brew --prefix)/share/udev/rules.d/90-utter.rules /etc/udev/rules.d/
           sudo udevadm control --reload-rules && sudo udevadm trigger

        2. Add yourself to the `input` group (needed for evdev key watching):
           sudo usermod -aG input $USER
           # Log out and back in for the group change to take effect.

        3. Download the speech model (~650 MB):
           download-model.sh

        4. Enable and start the services:
           systemctl --user daemon-reload
           systemctl --user enable --now utter-daemon utter-watcher
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/utter --version")
  end
end
