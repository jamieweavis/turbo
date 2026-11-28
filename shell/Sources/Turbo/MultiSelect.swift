import Foundation

/// The rows marked for a bulk action in a palette list or grid, by the rows' own ids so
/// they survive the list reordering or refreshing underneath.
///
/// Marking works like a Mac list: ⌘-click (or Tab) toggles one row, ⇧-click and ⇧-arrows
/// mark the run from an anchor (the last row moved to or toggled) to the target, growing
/// and shrinking as the target moves. Unlike a Mac list, plain arrows leave the marks
/// alone, since the palette's cursor is always on some row; Esc clears them.
struct Marks<ID: Hashable> {
    private(set) var ids: Set<ID> = []
    /// Where a ⇧ run starts, and where it currently ends.
    private var anchor: ID?
    private var head: ID?
    /// The marks outside the run in progress, which it's laid over.
    private var base: Set<ID> = []

    var isEmpty: Bool { ids.isEmpty }
    var count: Int { ids.count }

    func contains(_ id: ID) -> Bool {
        ids.contains(id)
    }

    /// ⌘-click or Tab.
    mutating func toggle(_ id: ID) {
        if ids.remove(id) == nil { ids.insert(id) }
        startRun(at: id)
    }

    /// The cursor moved without ⇧: the next run starts from there.
    mutating func cursorMoved(to id: ID?) {
        startRun(at: id)
    }

    /// ⇧-arrow from `cursor` to `target`. A cursor that isn't where the last run ended (the
    /// mouse moved it) starts a new run rather than stretching the old one.
    mutating func extend(from cursor: ID, to target: ID, in order: [ID]) {
        if anchor == nil || head != cursor { startRun(at: cursor) }
        markRun(to: target, in: order)
    }

    /// ⇧-click on `target`.
    mutating func extendFromAnchor(to target: ID, in order: [ID]) {
        if anchor == nil { startRun(at: target) }
        markRun(to: target, in: order)
    }

    mutating func selectAll(_ order: [ID]) {
        ids = Set(order)
        base = ids
    }

    /// Keeps the anchor, so a ⇧-click after deselecting still has somewhere to start.
    mutating func clear() {
        ids = []
        base = []
    }

    /// Drops marks for rows that are gone (deleted, or filtered out by the query), so bulk
    /// actions only ever touch rows that are showing. A lost anchor falls back to `cursor`.
    mutating func prune(to order: [ID], cursor: ID?) {
        let present = Set(order)
        ids.formIntersection(present)
        base.formIntersection(present)
        if anchor.map(present.contains) != true {
            anchor = cursor
            head = cursor
        }
    }

    private mutating func startRun(at id: ID?) {
        anchor = id
        head = id
        base = ids
    }

    private mutating func markRun(to target: ID, in order: [ID]) {
        guard let anchor, let from = order.firstIndex(of: anchor), let to = order.firstIndex(of: target) else {
            return
        }
        ids = base.union(order[min(from, to)...max(from, to)])
        head = target
    }
}

/// A palette browser whose rows can be marked for bulk actions.
@MainActor
protocol MultiSelecting: AnyObject {
    associatedtype MarkID: Hashable
    /// The rows' ids, in the order they're shown.
    var markIDs: [MarkID] { get }
    /// The cursor: the row the keyboard is on.
    var selection: Int { get set }
    var marks: Marks<MarkID> { get set }
}

extension MultiSelecting {
    var hasMarks: Bool { !marks.isEmpty }

    var isSelectionMarked: Bool {
        let ids = markIDs
        return ids.indices.contains(selection) && marks.contains(ids[selection])
    }

    var allMarked: Bool {
        let count = markIDs.count
        return count > 0 && marks.count == count
    }

    /// The rows an action applies to: the marked ones in the order shown, or with nothing
    /// marked, the one under the cursor.
    var targetIndices: [Int] {
        let ids = markIDs
        if marks.isEmpty {
            return ids.indices.contains(selection) ? [selection] : []
        }
        return ids.indices.filter { marks.contains(ids[$0]) }
    }

    func toggleMark(at index: Int) {
        let ids = markIDs
        guard ids.indices.contains(index) else { return }
        marks.toggle(ids[index])
    }

    /// After the cursor has moved from `previous` with ⇧ held.
    func extendMarks(from previous: Int) {
        let ids = markIDs
        guard ids.indices.contains(previous), ids.indices.contains(selection) else { return }
        marks.extend(from: ids[previous], to: ids[selection], in: ids)
    }

    /// ⇧-click.
    func extendMarks(toClicked index: Int) {
        let ids = markIDs
        guard ids.indices.contains(index) else { return }
        marks.extendFromAnchor(to: ids[index], in: ids)
    }

    func markAll() {
        marks.selectAll(markIDs)
    }

    func clearMarks() {
        marks.clear()
    }

    /// After the cursor has moved without ⇧.
    func cursorMoved() {
        let ids = markIDs
        marks.cursorMoved(to: ids.indices.contains(selection) ? ids[selection] : nil)
    }

    /// After the rows changed.
    func pruneMarks() {
        let ids = markIDs
        marks.prune(to: ids, cursor: ids.indices.contains(selection) ? ids[selection] : nil)
    }

    /// "3 selected", for the footer.
    var marksSummary: String { "\(marks.count) selected" }
}
