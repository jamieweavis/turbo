import AppKit
import Observation
import SwiftUI
import os

/// The App Icons switch and the list of icons the core found, which the core keeps applied.
@MainActor
@Observable
final class AppIconsSettings {
    var config: AppIconsConfig {
        didSet {
            if config != oldValue { save() }
        }
    }
    private(set) var icons: [AppIcon]
    private(set) var saveError: String?
    @ObservationIgnored private let core: Core
    @ObservationIgnored private var previews: [String: NSImage] = [:]

    init(core: Core) {
        self.core = core
        config = core.appIconsConfig()
        icons = core.appIcons()
    }

    var isBlocked: Bool {
        icons.contains { $0.status == .blocked }
    }

    /// Has the core look at the folder and the apps again; what it finds arrives in `iconsChanged`.
    func sync() {
        core.syncAppIcons()
    }

    func iconsChanged(_ icons: [AppIcon]) {
        self.icons = icons
        // A file can change under the same name, and the apps' own icons just changed.
        previews.removeAll()
        AppIcons.invalidate()
    }

    func preview(for icon: AppIcon) -> NSImage {
        if let image = previews[icon.iconPath] { return image }
        let image = NSImage(contentsOfFile: icon.iconPath) ?? NSWorkspace.shared.icon(for: .icns)
        previews[icon.iconPath] = image
        return image
    }

    func openFolder() {
        NSWorkspace.shared.open(URL(filePath: core.appIconsFolder(), directoryHint: .isDirectory))
    }

    private func save() {
        do {
            try core.setAppIconsConfig(appIcons: config)
            saveError = nil
        } catch {
            Logger.settings.error("couldn't save app icon settings: \(error, privacy: .public)")
            saveError = error.userMessage
        }
    }
}

/// Receives the core's callback on a background thread and forwards it to the main actor.
/// Both live for the app's lifetime, so holding `settings` strongly is fine.
final class AppIconsObserverBridge: AppIconsObserver {
    private let settings: AppIconsSettings

    init(settings: AppIconsSettings) {
        self.settings = settings
    }

    func appIconsChanged(icons: [AppIcon]) {
        DispatchQueue.main.async { [settings] in
            MainActor.assumeIsolated { settings.iconsChanged(icons) }
        }
    }
}

struct AppIconsPane: View {
    @Bindable var settings: AppIconsSettings

    var body: some View {
        Form {
            PaneHeaderSection(pane: .appIcons)

            if let error = settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            if settings.isBlocked {
                Section {
                    LabeledContent {
                        Button("Open System Settings…") {
                            if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AppBundles") {
                                NSWorkspace.shared.open(url)
                            }
                        }
                    } label: {
                        Label("macOS stopped Turbo changing an app", systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.yellow)
                        Text("Allow Turbo under Privacy & Security → App Management.")
                    }
                }
            }

            Section {
                if settings.icons.isEmpty {
                    Text("No icons yet.")
                        .foregroundStyle(.secondary)
                }
                ForEach(settings.icons, id: \.iconPath) { icon in
                    LabeledContent {
                        status(icon.status)
                    } label: {
                        Label {
                            Text(icon.name)
                        } icon: {
                            Image(nsImage: settings.preview(for: icon))
                                .resizable()
                                .frame(width: 28, height: 28)
                        }
                    }
                }
            } header: {
                HStack {
                    Text("Icons")
                    Spacer()
                    Button("Open Icons Folder") { settings.openFolder() }
                }
            } footer: {
                Text("Name each .icns file after its app: Spotify.icns goes on Spotify.app. Turbo puts an icon back when an update clears it, and switching this off gives every app its own icon again.")
            }
        }
        .formStyle(.grouped)
        .onAppear { settings.sync() }
        // Coming back from Finder after adding icons, or from System Settings after allowing it.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            settings.sync()
        }
    }

    @ViewBuilder
    private func status(_ status: AppIconStatus) -> some View {
        switch status {
        case .applied:
            Label("Applied", systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green)
        case .off:
            Text("Off").foregroundStyle(.secondary)
        case .appNotFound:
            Text("App not installed").foregroundStyle(.secondary)
        case .needsAdmin:
            Text("Needs an administrator").foregroundStyle(.secondary)
        case .blocked:
            Label("Blocked by macOS", systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.yellow)
        case .unreadable:
            Label("Not a readable icon", systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.yellow)
        }
    }
}
