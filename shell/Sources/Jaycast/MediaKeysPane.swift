import AppKit
import Observation
import SwiftUI
import os

/// The media key settings the UI edits, written through to the core, which saves them and
/// starts or stops the key tap.
@MainActor
@Observable
final class MediaKeysSettings {
    var config: MediaKeysConfig {
        didSet {
            if config != oldValue { save() }
        }
    }
    private(set) var saveError: String?
    /// Installed supported players, refreshed when the pane appears.
    private(set) var players: [MediaPlayer] = []
    /// Whether Jaycast may control the chosen player.
    private(set) var access: AutomationAccess = .notDetermined
    @ObservationIgnored private let core: Core

    init(core: Core) {
        self.core = core
        config = core.mediaKeysConfig()
    }

    var chosenPlayer: MediaPlayer? {
        players.first { $0.bundleId == config.player }
    }

    func refresh() {
        players = core.mediaPlayers()
        refreshAccess()
    }

    /// Asking macOS whether we may automate a player can block indefinitely (it waits on TCC and
    /// the player), so it runs on its own serial queue, one check at a time, never on the main
    /// thread or the shared concurrency pool. Requests made while one is in flight collapse into a
    /// single re-check of the latest player once it returns.
    @ObservationIgnored private let accessQueue = DispatchQueue(label: "com.jamieweavis.jaycast.media-access")
    @ObservationIgnored private var accessCheckRunning = false
    @ObservationIgnored private var accessCheckPending = false

    func refreshAccess() {
        guard !accessCheckRunning else {
            accessCheckPending = true
            return
        }
        accessCheckRunning = true
        let core = core
        let bundleID = config.player
        accessQueue.async { [weak self] in
            let access = core.mediaPlayerAccess(bundleId: bundleID)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { return }
                    self.accessCheckRunning = false
                    if self.accessCheckPending {
                        self.accessCheckPending = false
                        self.refreshAccess()
                    } else if bundleID == self.config.player {
                        self.access = access
                    }
                }
            }
        }
    }

    /// Shows macOS's Automation prompt, which blocks until answered, so off the main thread.
    func requestAccess() {
        let core = core
        let bundleID = config.player
        Task {
            let access = await Task.detached { core.requestMediaPlayerAccess(bundleId: bundleID) }.value
            self.access = access
        }
    }

    private func save() {
        do {
            try core.setMediaKeysConfig(mediaKeys: config)
            saveError = nil
        } catch {
            Logger.settings.error("couldn't save media key settings: \(error, privacy: .public)")
            saveError = error.userMessage
        }
        refreshAccess()
    }
}

struct MediaKeysPane: View {
    @Bindable var settings: MediaKeysSettings
    let isInputRunning: Bool

    var body: some View {
        Form {
            PaneHeaderSection(pane: .mediaKeys)

            if let error = settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                Picker(selection: $settings.config.target) {
                    Text("Last used music app").tag(MediaTarget.lastActive)
                    Text("Always the chosen app").tag(MediaTarget.fixed)
                } label: {
                    Text("Send keys to")
                    Text(settings.config.target == .lastActive
                         ? "The app you last played from, or the chosen app if none is open."
                         : "Other music apps keep running but don't get the keys.")
                }

                Picker(selection: $settings.config.player) {
                    ForEach(settings.players, id: \.bundleId) { player in
                        Label {
                            Text(player.name)
                        } icon: {
                            Image(nsImage: AppIcons.icon(forPath: player.path))
                        }
                        .tag(player.bundleId)
                    }
                    // Keeps the picker valid when the saved app isn't installed.
                    if settings.chosenPlayer == nil {
                        Text("Not installed").tag(settings.config.player)
                    }
                } label: {
                    Text(settings.config.target == .lastActive ? "Fallback app" : "App")
                }

                Picker(selection: $settings.config.whenNotRunning) {
                    Text("Open it").tag(WhenNotRunning.launch)
                    Text("Do nothing").tag(WhenNotRunning.ignore)
                    Text("Let macOS handle the key").tag(WhenNotRunning.passThrough)
                } label: {
                    Text("When it isn't open")
                    if settings.config.whenNotRunning == .passThrough {
                        Text("macOS may open Music.")
                    }
                }
            } footer: {
                footer
            }

            if let player = settings.chosenPlayer {
                Section {
                    LabeledContent("Control \(player.name)") { accessStatus(player) }
                } header: {
                    Text("Permission")
                } footer: {
                    Text("Every music app the keys go to needs its own permission.")
                }
            }
        }
        .formStyle(.grouped)
        .onAppear { settings.refresh() }
        // Coming back from System Settings after allowing it.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            settings.refresh()
        }
    }

    @ViewBuilder
    private func accessStatus(_ player: MediaPlayer) -> some View {
        switch settings.access {
        case .granted:
            Label("Allowed", systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green)
        case .notDetermined:
            Button("Allow…") { settings.requestAccess() }
        case .denied:
            Button("Open System Settings…") {
                if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation") {
                    NSWorkspace.shared.open(url)
                }
            }
        case .notRunning:
            Text("macOS asks the first time a key goes to \(player.name).")
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var footer: some View {
        if settings.config.enabled && !isInputRunning {
            Text("Media keys work once Accessibility is granted.")
                .foregroundStyle(.red)
        } else if settings.players.isEmpty {
            Label("No supported music app is installed (Spotify, Music or VLC).",
                  systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.yellow)
        } else {
            Text("Play/pause, next and previous go to Spotify, Music or VLC instead of whatever macOS picks. AirPods and Control Center keep macOS's own behaviour.")
        }
    }
}
