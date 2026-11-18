import AVFoundation
import AppKit
import Observation
import SwiftUI
import os

// MARK: - Session

/// The capture session for Camera Preview. The core decides which camera to show; this only
/// runs it. AVCaptureSession calls block, so they run on a serial queue of their own, which
/// also keeps a stop queued behind a start in order.
final class CameraSession: @unchecked Sendable {
    /// Read by the preview layer. Only configured on `queue`.
    let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "com.jamieweavis.backpack.camera")
    /// Only touched on `queue`.
    private var input: AVCaptureDeviceInput?

    /// Shows `deviceID`, replacing the camera being shown, and starts the session if needed.
    func start(deviceID: String, completion: @escaping @Sendable (String?) -> Void) {
        queue.async { [self] in
            guard let device = AVCaptureDevice(uniqueID: deviceID) else {
                completion("The camera was disconnected.")
                return
            }
            if input?.device.uniqueID != deviceID {
                let next: AVCaptureDeviceInput
                do {
                    next = try AVCaptureDeviceInput(device: device)
                } catch {
                    completion(error.localizedDescription)
                    return
                }
                session.beginConfiguration()
                if let input { session.removeInput(input) }
                input = nil
                if session.canAddInput(next) {
                    session.addInput(next)
                    input = next
                }
                if session.canSetSessionPreset(.high) { session.sessionPreset = .high }
                session.commitConfiguration()
                guard input != nil else {
                    completion("Couldn't use \(device.localizedName).")
                    return
                }
            }
            if !session.isRunning { session.startRunning() }
            completion(nil)
        }
    }

    /// Stops the session and lets go of the camera, so its light goes off.
    func stop() {
        queue.async { [self] in
            if session.isRunning { session.stopRunning() }
            if let input {
                session.beginConfiguration()
                session.removeInput(input)
                session.commitConfiguration()
            }
            input = nil
        }
    }
}

// MARK: - State

enum CameraPreviewState: Equatable {
    /// Not showing.
    case idle
    /// Asking for access, or waiting for the camera's first frames.
    case starting
    case running
    /// The prompt was refused, or access was turned off in System Settings.
    case denied
    /// A management profile or parental controls block the camera.
    case restricted
    case noCamera
    case failed(String)
}

extension CameraPreviewState {
    /// Shown instead of the feed: no camera, no access, or an error.
    var isMessage: Bool {
        switch self {
        case .idle, .starting, .running: false
        case .denied, .restricted, .noCamera, .failed: true
        }
    }
}

/// Runs the preview while the palette shows it: asks for access the first time, starts the
/// camera the core picks, follows cameras coming and going, and stops when it's hidden.
@MainActor
@Observable
final class CameraPreview {
    private(set) var state = CameraPreviewState.idle
    /// The camera being shown (or starting).
    private(set) var camera: CameraDevice?
    /// Connected cameras, for the ⌘K menu; refreshed whenever one comes or goes.
    private(set) var cameras: [CameraDevice] = []
    @ObservationIgnored let session = CameraSession()
    @ObservationIgnored private let core: Core
    @ObservationIgnored private var isActive = false
    /// Bumped on every start and stop, so a late answer from an earlier one is ignored.
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private var observers: [NSObjectProtocol] = []

    init(core: Core) {
        self.core = core
    }

    func start() {
        guard !isActive else { return }
        isActive = true
        observeDevices()
        generation += 1
        let generation = generation
        switch core.cameraAccess() {
        case .authorized:
            show()
        case .notDetermined:
            state = .starting
            Task { [weak self, core] in
                let granted = await core.requestCameraAccess()
                guard let self, self.isActive, self.generation == generation else { return }
                Logger.palette.info("camera access \(granted ? "granted" : "refused", privacy: .public)")
                if granted { self.show() } else { self.state = .denied }
            }
        case .denied:
            state = .denied
        case .restricted:
            state = .restricted
        }
    }

    func stop() {
        guard isActive else { return }
        isActive = false
        generation += 1
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
        observers = []
        session.stop()
        state = .idle
        camera = nil
        cameras = []
    }

    func select(_ id: String) throws {
        try core.selectCamera(id: id)
        show()
    }


    /// Shows the camera the core picks now, switching if it changed; nothing to do if it's
    /// already showing.
    func show() {
        guard isActive, core.cameraAccess() == .authorized else { return }
        cameras = core.cameras()
        guard let current = cameras.first(where: \.selected) else {
            camera = nil
            state = .noCamera
            session.stop()
            return
        }
        if current.id == camera?.id, state == .running { return }
        camera = current
        state = .starting
        generation += 1
        let generation = generation
        session.start(deviceID: current.id) { error in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { [weak self] in
                    guard let self, self.isActive, self.generation == generation else { return }
                    if let error {
                        Logger.palette.error("couldn't start the camera: \(error, privacy: .public)")
                        self.state = .failed(error)
                    } else {
                        self.state = .running
                    }
                }
            }
        }
    }

    /// A camera plugged in or out (or an iPhone coming into or out of range) may change
    /// which one should show; the core decides.
    private func observeDevices() {
        let center = NotificationCenter.default
        let names: [Notification.Name] = [AVCaptureDevice.wasConnectedNotification, AVCaptureDevice.wasDisconnectedNotification]
        observers = names.map { name in
            center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    // A disconnect leaves the shown camera's input dead; start over.
                    if let id = self.camera?.id, AVCaptureDevice(uniqueID: id)?.isConnected != true {
                        self.camera = nil
                    }
                    self.show()
                }
            }
        }
        observers.append(center.addObserver(forName: AVCaptureSession.runtimeErrorNotification, object: session.session, queue: .main) { [weak self] notification in
            let error = notification.userInfo?[AVCaptureSessionErrorKey] as? Error
            let message = error?.localizedDescription ?? "The camera stopped."
            MainActor.assumeIsolated {
                guard let self, self.isActive else { return }
                Logger.palette.error("camera session error: \(message, privacy: .public)")
                self.state = .failed(message)
            }
        })
    }
}

// MARK: - Palette mode

extension PaletteLayout {
    /// Space around the feed, inside the palette.
    static let cameraInset: CGFloat = 10
    /// 16:9 across the palette's width, inside the inset.
    static let cameraFeedHeight = ((width(for: .camera) - cameraInset * 2) * 9 / 16).rounded()
    static let cameraCornerRadius = cornerRadius - cameraInset
}

struct CameraModeView: View {
    let model: PaletteModel

    private var preview: CameraPreview { model.camera }

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: PaletteLayout.cameraCornerRadius, style: .continuous)
                .fill(.black.opacity(0.85))
            if preview.camera != nil, preview.state == .running || preview.state == .starting {
                CameraFeed(session: preview.session.session, mirrored: model.cameraMirrored)
                    .opacity(preview.state == .running ? 1 : 0)
                    .animation(.easeOut(duration: 0.2), value: preview.state)
            }
            placeholder
        }
        .clipShape(.rect(cornerRadius: PaletteLayout.cameraCornerRadius, style: .continuous))
        .overlay(alignment: .bottom) {
            // The feed (and the space it's loading into) gets the whole frame; settings
            // only come back with a message.
            FloatingActionBar(onShowSettings: model.onShowSettings, actions: actions,
                              showsSettings: preview.state.isMessage)
        }
        .environment(\.colorScheme, .dark)
        .frame(height: PaletteLayout.cameraFeedHeight)
        .padding([.horizontal, .bottom], PaletteLayout.cameraInset)
    }

    private var actions: [FloatingAction] {
        preview.state == .running ? [.showActions(model)] : []
    }

    @ViewBuilder
    private var placeholder: some View {
        switch preview.state {
        case .idle, .running:
            EmptyView()
        case .starting:
            ProgressView().controlSize(.small)
        case .denied:
            CameraMessage(
                symbol: "video.slash",
                title: "Backpack can't use the camera",
                detail: "Allow it in System Settings → Privacy & Security → Camera.",
                button: ("Open Camera Settings", model.openCameraPrivacySettings)
            )
        case .restricted:
            CameraMessage(symbol: "lock", title: "The camera is restricted",
                          detail: "A management profile or Screen Time doesn't allow it on this Mac.")
        case .noCamera:
            CameraMessage(symbol: "web.camera", title: "No camera connected",
                          detail: "Plug one in, or bring your iPhone close to use it as a camera.")
        case .failed(let message):
            CameraMessage(symbol: "exclamationmark.triangle", title: "The camera isn't working", detail: message)
        }
    }
}

private struct CameraMessage: View {
    let symbol: String
    let title: String
    let detail: String
    var button: (String, () -> Void)?

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 30))
                .foregroundStyle(.secondary)
            Text(title).font(.headline)
            Text(detail)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            if let button {
                Button(button.0, action: button.1)
                    .padding(.top, 4)
            }
        }
        .padding(.horizontal, 40)
        // Clear of the floating action bar.
        .padding(.bottom, PaletteLayout.actionBarHeight / 2)
    }
}

/// The live feed, filling its frame (cropping rather than letterboxing).
private struct CameraFeed: NSViewRepresentable {
    let session: AVCaptureSession
    let mirrored: Bool

    func makeNSView(context: Context) -> CameraFeedView {
        let view = CameraFeedView()
        view.previewLayer.session = session
        view.mirrored = mirrored
        return view
    }

    func updateNSView(_ view: CameraFeedView, context: Context) {
        view.mirrored = mirrored
    }

    static func dismantleNSView(_ view: CameraFeedView, coordinator: ()) {
        view.previewLayer.session = nil
    }
}

final class CameraFeedView: NSView {
    let previewLayer = AVCaptureVideoPreviewLayer()

    /// Flips the layer rather than the connection, which only exists once the session has
    /// a camera.
    var mirrored = false {
        didSet {
            guard mirrored != oldValue else { return }
            needsLayout = true
        }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        previewLayer.videoGravity = .resizeAspectFill
        layer?.addSublayer(previewLayer)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        previewLayer.setAffineTransform(mirrored ? CGAffineTransform(scaleX: -1, y: 1) : .identity)
        previewLayer.frame = bounds
        CATransaction.commit()
    }
}

extension CameraKind {
    var symbol: String {
        switch self {
        case .builtIn: "laptopcomputer"
        case .external: "web.camera"
        case .continuity: "iphone"
        }
    }
}
