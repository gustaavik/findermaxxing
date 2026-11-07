import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// One folder listing (or search). Layout and input routing only: listing and
// search decisions belong to FolderViewController, file operations to the
// window's OperationsController, previews to PreviewController, and
// selection state to the list model.
FocusScope {
    id: root

    required property ServicesHolder services
    required property OperationsController operations
    required property SidebarController sidebar
    readonly property alias controller: controller
    readonly property var entries: controller.model

    readonly property int rowHeight: 24
    readonly property int iconSize: 16
    readonly property int sizeColumnWidth: 90
    readonly property int modifiedColumnWidth: 170
    readonly property int columnSpacing: 12
    readonly property int sidePadding: 10
    readonly property int folderColumnWidth: 180
    readonly property int previewMinimumWidth: 220
    readonly property int previewPreferredWidth: 340
    readonly property int searchFieldWidth: 220
    readonly property int searchDelayMs: 250
    readonly property bool isMac: Qt.platform.os === "osx"
    readonly property string newFolderName: isMac ? qsTr("untitled folder") : qsTr("New Folder")

    // Row being renamed inline, or -1.
    property int renamingRow: -1
    // Locations to select (and for new folders, rename) once the watcher shows them.
    property string pendingSelection: ""
    property bool pendingRename: false
    property bool previewVisible: false
    readonly property int appearance: Qt.styleHints.colorScheme === Qt.ColorScheme.Dark
                                      ? PreviewController.Dark : PreviewController.Light

    // Window-wide tools, opened by Main.
    signal findDuplicatesRequested(string location, string label)
    signal diskUsageRequested(string location)

    function selection() {
        return entries.selectedLocations()
    }

    // The selected folder, or the one being viewed.
    function toolFolder() {
        const row = list.currentIndex
        const folder = entries.selectedCount === 1 && entries.isSelected(row) && entries.isContainerAt(row)
        return folder ? { location: entries.locationAt(row), label: entries.nameAt(row) }
                      : { location: controller.locationId, label: controller.title }
    }

    function selectedImages() {
        return selection().filter(location => root.operations.isImage(location))
    }

    // Our own locations ride along with the file URLs, so a drop inside the
    // app moves the real items rather than cached copies of remote ones.
    readonly property string locationsMime: "application/x-findermaxxing-locations"
    property bool dragPending: false
    property bool pointerDown: false

    // Local files for other apps can take a moment (remote items download
    // first), so the drag starts when they are ready, if the button is still down.
    function beginDrag() {
        if (entries.selectedCount === 0) {
            return
        }
        dragPending = true
        operations.prepareDrag(selection())
    }

    function dropOn(drop, index) {
        const target = index >= 0 && entries.isContainerAt(index) ? entries.locationAt(index) : controller.locationId
        operations.dropItems(drop.urls.map(url => url.toString()), drop.getDataAsString(locationsMime) ?? "",
                             target, OperationsController.FollowKeys)
        drop.accept()
    }

    function startRename(row) {
        if (entries.selectedCount > 1 && entries.isSelected(row)) {
            bulkRename.openFor(selection())
            return
        }
        if (row >= 0) {
            list.currentIndex = row
            entries.selectOnly(row)
            renamingRow = row
        }
    }

    function finishRename(location, newName, oldName) {
        renamingRow = -1
        list.forceActiveFocus()
        if (newName !== oldName) {
            operations.rename(location, newName)
        }
    }

    function runSearch() {
        if (searchBar.contents) {
            controller.searchContent(searchField.text)
        } else {
            controller.search(searchField.text, searchBar.kind, searchBar.date, searchBar.size)
        }
    }

    function endSearch() {
        searchField.text = ""
        controller.clearSearch()
        list.forceActiveFocus()
    }

    function updatePreview() {
        if (previewVisible && list.currentIndex >= 0) {
            preview.show(entries.locationAt(list.currentIndex), appearance)
        } else if (previewVisible) {
            preview.clear()
        }
        preview.followSelection(selection())
    }

    function quickLook() {
        // Quick Look on macOS; elsewhere Space toggles the preview pane.
        if (!preview.toggleQuickLook(selection())) {
            previewVisible = !previewVisible
        }
    }

    function trashSelection() {
        // No Trash here (S3): never delete silently; ask for a permanent delete.
        if (controller.canTrash) {
            operations.moveToTrash(selection())
        } else {
            confirmPermanentDelete()
        }
    }

    function selectedOrCurrentFolder() {
        const row = list.currentIndex
        return entries.selectedCount === 1 && entries.isSelected(row) ? entries.locationAt(row) : controller.locationId
    }

    function confirmPermanentDelete() {
        const items = selection()
        if (items.length > 0) {
            confirmDelete.locations = items
            confirmDelete.open()
        }
    }

    function applyPendingSelection() {
        if (pendingSelection === "") {
            return
        }
        const row = entries.rowOfLocation(pendingSelection)
        if (row < 0) {
            return
        }
        entries.selectOnly(row)
        list.currentIndex = row
        if (pendingRename) {
            renamingRow = row
        }
        pendingSelection = ""
        pendingRename = false
    }

    focus: true

    FolderViewController {
        id: controller
        services: root.services
        Component.onCompleted: openHome()
        onOpenRequested: location => root.operations.open(location)
        onLocationIdChanged: root.renamingRow = -1
    }

    Connections {
        target: root.operations
        enabled: root.visible
        function onFolderCreated(location) {
            root.pendingSelection = location
            root.pendingRename = true
            root.applyPendingSelection()
        }
        function onRenamed(location) {
            root.pendingSelection = location
            root.pendingRename = false
            root.applyPendingSelection()
        }
    }

    Connections {
        target: root.entries
        function onRowsInserted() { root.applyPendingSelection() }
        function onModelReset() { root.applyPendingSelection() }
        function onSelectionChanged() { root.updatePreview() }
    }

    Connections {
        target: controller
        // Running a saved search fills in its text.
        function onSearchTextChanged() {
            if (searchField.text !== controller.searchText) {
                searchField.text = controller.searchText
            }
        }
    }

    onPreviewVisibleChanged: updatePreview()

    PreviewController {
        id: preview
        services: root.services
    }

    InfoDialog {
        id: info
        services: root.services
    }

    BulkRenameDialog {
        id: bulkRename
        services: root.services
        operations: root.operations
    }

    CompressDialog {
        id: compress
        operations: root.operations
    }

    ChecksumDialog {
        id: checksums
        services: root.services
    }

    ImageToolsDialog {
        id: imageTools
        services: root.services
        operations: root.operations
    }

    PermissionsDialog {
        id: permissions
        services: root.services
        operations: root.operations
    }

    SecureDeleteDialog {
        id: secureDelete
        operations: root.operations
    }

    Timer {
        id: searchDelay
        interval: root.searchDelayMs
        onTriggered: root.runSearch()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        ToolBar {
            Layout.fillWidth: true

            RowLayout {
                anchors.fill: parent

                ToolButton {
                    text: "‹"
                    enabled: controller.canGoBack
                    onClicked: controller.goBack()
                    Accessible.name: qsTr("Back")
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Back")
                }
                ToolButton {
                    text: "›"
                    enabled: controller.canGoForward
                    onClicked: controller.goForward()
                    Accessible.name: qsTr("Forward")
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Forward")
                }
                ToolButton {
                    text: "↑"
                    enabled: controller.canGoUp
                    onClicked: controller.goUp()
                    Accessible.name: qsTr("Enclosing folder")
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Enclosing folder")
                }
                TextField {
                    id: locationField
                    Layout.fillWidth: true
                    text: controller.location
                    selectByMouse: true
                    onAccepted: {
                        controller.openLocation(text)
                        list.forceActiveFocus()
                    }
                    Accessible.name: qsTr("Location")
                }
                ToolButton {
                    text: controller.showHidden ? qsTr("Hide hidden") : qsTr("Show hidden")
                    onClicked: controller.toggleHiddenFiles()
                    Accessible.name: text
                }
                ToolButton {
                    text: qsTr("Preview")
                    checkable: true
                    checked: root.previewVisible
                    onToggled: root.previewVisible = checked
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Show or hide the preview pane")
                    Accessible.name: qsTr("Preview pane")
                }
                TextField {
                    id: searchField
                    Layout.preferredWidth: root.searchFieldWidth
                    placeholderText: qsTr("Search")
                    selectByMouse: true
                    Accessible.name: qsTr("Search in %1").arg(controller.title)
                    onTextEdited: searchDelay.restart()
                    onAccepted: {
                        searchDelay.stop()
                        root.runSearch()
                    }
                    Keys.onEscapePressed: root.endSearch()
                }
            }
        }

        SearchBar {
            id: searchBar
            Layout.fillWidth: true
            visible: controller.searching
            controller: controller
            onChanged: root.runSearch()
            onDone: root.endSearch()
        }

        SplitView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            orientation: Qt.Horizontal

        ColumnLayout {
            SplitView.fillWidth: true
            spacing: 0

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: root.rowHeight
            color: palette.alternateBase

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: root.sidePadding + root.iconSize + root.columnSpacing
                anchors.rightMargin: root.sidePadding
                spacing: root.columnSpacing

                ColumnHeader {
                    Layout.fillWidth: true
                    label: qsTr("Name")
                    column: FolderViewController.Name
                }
                Label {
                    Layout.preferredWidth: root.folderColumnWidth
                    visible: controller.searching
                    text: qsTr("Folder")
                    elide: Text.ElideRight
                    Accessible.role: Accessible.ColumnHeader
                }
                ColumnHeader {
                    Layout.preferredWidth: root.sizeColumnWidth
                    label: qsTr("Size")
                    column: FolderViewController.Size
                    horizontalAlignment: Text.AlignRight
                }
                ColumnHeader {
                    Layout.preferredWidth: root.modifiedColumnWidth
                    label: qsTr("Date Modified")
                    column: FolderViewController.Modified
                }
            }
        }

        ListView {
            id: list

            function moveCurrent(step, extend) {
                if (count === 0) {
                    return
                }
                const next = Math.max(0, Math.min(count - 1, currentIndex + step))
                currentIndex = next
                if (extend) {
                    root.entries.extendSelectionTo(next)
                } else {
                    root.entries.selectOnly(next)
                }
            }

            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            focus: true
            reuseItems: true
            boundsBehavior: Flickable.StopAtBounds
            keyNavigationEnabled: false
            highlightMoveDuration: 0
            model: root.entries
            Accessible.role: Accessible.List
            Accessible.name: qsTr("Contents of %1").arg(controller.title)

            ScrollBar.vertical: ScrollBar {}

            // Clicks on empty space below the rows.
            MouseArea {
                anchors.fill: parent
                z: -1
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                onClicked: mouse => {
                    root.entries.clearSelection()
                    list.forceActiveFocus()
                    if (mouse.button === Qt.RightButton) {
                        contextMenu.popup()
                    }
                }
            }

            delegate: ItemDelegate {
                id: row

                required property int index
                required property string name
                required property string location
                required property string iconName
                required property string sizeText
                required property var modified
                required property bool isContainer
                required property bool isHidden
                required property bool selected
                required property string folderText

                width: ListView.view.width
                height: root.rowHeight
                padding: 0
                leftPadding: root.sidePadding
                rightPadding: root.sidePadding
                highlighted: selected
                opacity: isHidden ? 0.6 : 1.0

                Accessible.role: Accessible.ListItem
                Accessible.name: name
                Accessible.selected: selected

                contentItem: RowLayout {
                    spacing: root.columnSpacing

                    FileIcon {
                        Layout.preferredWidth: root.iconSize
                        Layout.preferredHeight: root.iconSize
                        kind: row.iconName
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: root.renamingRow !== row.index
                        text: row.name
                        elide: Text.ElideMiddle
                        color: row.highlighted ? palette.highlightedText : palette.text
                    }
                    Loader {
                        Layout.fillWidth: true
                        active: root.renamingRow === row.index
                        visible: active
                        sourceComponent: TextField {
                            text: row.name
                            Accessible.name: qsTr("New name for %1").arg(row.name)
                            Component.onCompleted: {
                                // Like Finder: preselect the name without its extension.
                                const dot = row.name.lastIndexOf(".")
                                select(0, dot > 0 && !row.isContainer ? dot : row.name.length)
                                forceActiveFocus()
                            }
                            onAccepted: root.finishRename(row.location, text, row.name)
                            onActiveFocusChanged: if (!activeFocus && root.renamingRow === row.index) {
                                root.finishRename(row.location, text, row.name)
                            }
                            Keys.onEscapePressed: {
                                root.renamingRow = -1
                                list.forceActiveFocus()
                            }
                        }
                    }
                    Label {
                        Layout.preferredWidth: root.folderColumnWidth
                        visible: controller.searching
                        text: row.folderText
                        elide: Text.ElideMiddle
                        color: row.highlighted ? palette.highlightedText : palette.placeholderText
                    }
                    Label {
                        Layout.preferredWidth: root.sizeColumnWidth
                        text: row.isContainer ? "--" : row.sizeText
                        horizontalAlignment: Text.AlignRight
                        color: row.highlighted ? palette.highlightedText : palette.placeholderText
                    }
                    Label {
                        Layout.preferredWidth: root.modifiedColumnWidth
                        text: row.modified ? row.modified.toLocaleString(Qt.locale(), Locale.ShortFormat) : ""
                        elide: Text.ElideRight
                        color: row.highlighted ? palette.highlightedText : palette.placeholderText
                    }
                }

                MouseArea {
                    readonly property bool dragging: drag.active

                    anchors.fill: parent
                    enabled: root.renamingRow !== row.index
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    drag.target: dragProxy
                    onDraggingChanged: if (dragging) root.beginDrag()
                    onReleased: root.pointerDown = false
                    onPressed: mouse => {
                        root.pointerDown = mouse.button === Qt.LeftButton
                        list.forceActiveFocus()
                        list.currentIndex = row.index
                        if (mouse.button === Qt.RightButton) {
                            if (!row.selected) {
                                root.entries.selectOnly(row.index)
                            }
                        } else if (mouse.modifiers & Qt.ShiftModifier) {
                            root.entries.extendSelectionTo(row.index)
                        } else if (mouse.modifiers & Qt.ControlModifier) {
                            root.entries.toggleSelected(row.index)
                        } else if (!row.selected) {
                            root.entries.selectOnly(row.index)
                        }
                    }
                    onClicked: mouse => {
                        if (mouse.button === Qt.RightButton) {
                            contextMenu.popup()
                        } else if (!(mouse.modifiers & (Qt.ShiftModifier | Qt.ControlModifier))) {
                            root.entries.selectOnly(row.index)
                        }
                    }
                    onDoubleClicked: mouse => {
                        if (mouse.button === Qt.LeftButton) {
                            controller.activate(row.index)
                        }
                    }
                }
            }

            onCurrentIndexChanged: root.updatePreview()

            DropArea {
                anchors.fill: parent
                onEntered: drag => drag.accept()
                onDropped: drop => root.dropOn(drop, list.indexAt(drop.x + list.contentX, drop.y + list.contentY))
            }

            Keys.onPressed: event => {
                const extend = (event.modifiers & Qt.ShiftModifier) !== 0
                switch (event.key) {
                case Qt.Key_Up:
                    list.moveCurrent(-1, extend)
                    break
                case Qt.Key_Down:
                    list.moveCurrent(1, extend)
                    break
                case Qt.Key_Home:
                    list.moveCurrent(-list.count, extend)
                    break
                case Qt.Key_End:
                    list.moveCurrent(list.count, extend)
                    break
                case Qt.Key_Return:
                case Qt.Key_Enter:
                    // Finder renames on Return; elsewhere Return opens.
                    if (root.isMac) {
                        root.startRename(list.currentIndex)
                    } else {
                        controller.activate(list.currentIndex)
                    }
                    break
                case Qt.Key_Space:
                    root.quickLook()
                    break
                case Qt.Key_Escape:
                    if (controller.searching) {
                        root.endSearch()
                    } else {
                        root.entries.clearSelection()
                    }
                    break
                default:
                    return
                }
                event.accepted = true
            }

            Label {
                anchors.centerIn: parent
                width: parent.width * 0.8
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                visible: controller.errorMessage !== "" || (!controller.loading && list.count === 0)
                text: controller.errorMessage !== "" ? controller.errorMessage : qsTr("This folder is empty.")
                color: palette.placeholderText
            }
        }

        }

            PreviewPane {
                visible: root.previewVisible
                SplitView.minimumWidth: root.previewMinimumWidth
                SplitView.preferredWidth: root.previewPreferredWidth
                preview: preview
            }
        }

        ToolBar {
            Layout.fillWidth: true
            position: ToolBar.Footer

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: root.sidePadding
                anchors.rightMargin: root.sidePadding

                BusyIndicator {
                    Layout.preferredWidth: root.iconSize
                    Layout.preferredHeight: root.iconSize
                    running: controller.loading
                    visible: running
                }
                Label {
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    readonly property string items: controller.itemCount === 1
                                                    ? qsTr("1 item") : qsTr("%1 items").arg(controller.itemCount)
                    text: root.entries.selectedCount > 0
                          ? qsTr("%1 of %2 selected").arg(root.entries.selectedCount).arg(items)
                          : items
                    Accessible.name: text
                }
            }
        }
    }

    // Carries the drag: Drag.Automatic hands it to the OS, so it can leave
    // the window and land in Finder, a mail, or another app.
    Item {
        id: dragProxy
        width: 1
        height: 1
        Drag.dragType: Drag.Automatic
        Drag.supportedActions: Qt.CopyAction | Qt.MoveAction
        Drag.onDragFinished: {
            Drag.active = false
            x = 0
            y = 0
        }
    }

    Connections {
        target: root.operations
        function onDragReady(urls, internal) {
            if (!root.dragPending) {
                return
            }
            root.dragPending = false
            if (!root.pointerDown || urls.length === 0 && internal === "") {
                return
            }
            dragProxy.Drag.mimeData = { "text/uri-list": urls, [root.locationsMime]: internal }
            dragProxy.Drag.active = true
        }
    }

    Menu {
        id: contextMenu

        property bool pasteAvailable: false
        readonly property bool hasSelection: root.entries.selectedCount > 0
        readonly property bool singleSelection: root.entries.selectedCount === 1

        popupType: Popup.Native
        onAboutToShow: pasteAvailable = root.operations.canPaste()

        MenuItem {
            text: qsTr("Open")
            enabled: contextMenu.singleSelection
            onTriggered: controller.activate(list.currentIndex)
        }
        MenuSeparator {}
        MenuItem {
            text: contextMenu.singleSelection ? qsTr("Rename") : qsTr("Rename %1 Items…").arg(root.entries.selectedCount)
            enabled: contextMenu.hasSelection
            onTriggered: root.startRename(list.currentIndex)
        }
        MenuItem {
            text: qsTr("Get Info…")
            visible: controller.remote
            height: visible ? implicitHeight : 0
            enabled: contextMenu.singleSelection
            onTriggered: info.openFor(root.entries.locationAt(list.currentIndex), root.entries.nameAt(list.currentIndex))
        }
        MenuItem {
            text: qsTr("Add to Sidebar")
            onTriggered: root.sidebar.addFavorite(root.selectedOrCurrentFolder())
        }
        MenuItem {
            text: qsTr("Keep Available Offline")
            visible: controller.offlineCapable
            height: visible ? implicitHeight : 0
            enabled: contextMenu.hasSelection
            onTriggered: root.operations.makeAvailableOffline(root.selection())
        }
        MenuItem {
            text: qsTr("Remove Offline Copy")
            visible: controller.offlineCapable
            height: visible ? implicitHeight : 0
            enabled: contextMenu.singleSelection
            onTriggered: root.operations.removeOfflineCopy(root.entries.locationAt(list.currentIndex))
        }
        MenuItem {
            text: qsTr("Quick Look")
            enabled: contextMenu.hasSelection
            onTriggered: root.quickLook()
        }
        MenuItem {
            text: qsTr("Copy")
            enabled: contextMenu.hasSelection
            onTriggered: root.operations.copyToClipboard(root.selection())
        }
        MenuItem {
            text: qsTr("Paste")
            enabled: contextMenu.pasteAvailable
            onTriggered: root.operations.paste(controller.locationId, OperationsController.Copy)
        }
        MenuItem {
            text: qsTr("Move Here")
            enabled: contextMenu.pasteAvailable
            onTriggered: root.operations.paste(controller.locationId, OperationsController.Move)
        }
        Menu {
            title: qsTr("Tools")
            popupType: Popup.Native

            MenuItem {
                text: qsTr("Compress…")
                enabled: contextMenu.hasSelection
                onTriggered: compress.openFor(root.selection())
            }
            MenuItem {
                text: qsTr("Extract Here")
                enabled: contextMenu.singleSelection && root.operations.canExtract(root.entries.locationAt(list.currentIndex))
                onTriggered: root.operations.extract(root.entries.locationAt(list.currentIndex))
            }
            MenuItem {
                text: qsTr("Checksums…")
                enabled: contextMenu.singleSelection && !root.entries.isContainerAt(list.currentIndex)
                onTriggered: checksums.openFor(root.entries.locationAt(list.currentIndex))
            }
            MenuItem {
                text: qsTr("Convert or Resize Images…")
                enabled: contextMenu.hasSelection && root.selectedImages().length > 0
                onTriggered: imageTools.openFor(root.selectedImages(), false)
            }
            MenuItem {
                text: qsTr("Image Metadata…")
                enabled: contextMenu.hasSelection && root.selectedImages().length > 0
                onTriggered: imageTools.openFor(root.selectedImages(), true)
            }
            MenuItem {
                text: qsTr("Permissions…")
                enabled: contextMenu.hasSelection
                onTriggered: permissions.openFor(root.selection())
            }
            MenuSeparator {}
            MenuItem {
                text: qsTr("Find Duplicates…")
                onTriggered: {
                    const folder = root.toolFolder()
                    root.findDuplicatesRequested(folder.location, folder.label)
                }
            }
            MenuItem {
                text: qsTr("Disk Usage…")
                onTriggered: root.diskUsageRequested(root.toolFolder().location)
            }
        }
        MenuSeparator {}
        MenuItem {
            text: qsTr("Move to Trash")
            visible: controller.canTrash
            height: visible ? implicitHeight : 0
            enabled: contextMenu.hasSelection
            onTriggered: root.trashSelection()
        }
        MenuItem {
            text: qsTr("Delete Immediately…")
            enabled: contextMenu.hasSelection
            onTriggered: root.confirmPermanentDelete()
        }
        MenuItem {
            text: qsTr("Securely Delete…")
            visible: !controller.remote
            height: visible ? implicitHeight : 0
            enabled: contextMenu.hasSelection
            onTriggered: secureDelete.openFor(root.selection())
        }
        MenuSeparator {}
        MenuItem {
            text: qsTr("New Folder")
            onTriggered: root.operations.newFolder(controller.locationId, root.newFolderName)
        }
        MenuItem {
            text: root.operations.canUndo ? root.operations.undoText : qsTr("Undo")
            enabled: root.operations.canUndo
            onTriggered: root.operations.undo()
        }
        MenuItem {
            text: root.operations.canRedo ? root.operations.redoText : qsTr("Redo")
            enabled: root.operations.canRedo
            onTriggered: root.operations.redo()
        }
    }

    Dialog {
        id: confirmDelete

        property var locations: []

        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(440, parent.width - 32)
        modal: true
        title: locations.length === 1
               ? qsTr("Delete “%1” immediately?").arg(root.entries.nameAt(root.entries.rowOfLocation(locations[0])))
               : qsTr("Delete %1 items immediately?").arg(locations.length)

        Label {
            width: parent.width
            wrapMode: Text.Wrap
            text: controller.canTrash ? qsTr("You can't undo this action.")
                                      : qsTr("This location has no Trash. You can't undo this action.")
        }

        footer: DialogButtonBox {
            Button {
                text: qsTr("Cancel")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
            Button {
                text: qsTr("Delete")
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }
        onAccepted: root.operations.deletePermanently(locations)
    }

    component ColumnHeader: Label {
        id: header

        required property string label
        required property int column
        readonly property bool active: controller.sortColumn === column

        text: active ? label + (controller.sortAscending ? "  ▲" : "  ▼") : label
        font.bold: active
        elide: Text.ElideRight
        Accessible.role: Accessible.ColumnHeader
        Accessible.name: qsTr("Sort by %1").arg(label)

        TapHandler {
            onTapped: controller.sortBy(header.column)
        }
    }

    // Navigation
    Shortcut {
        enabled: root.visible
        sequences: [StandardKey.Back]
        onActivated: controller.goBack()
    }
    Shortcut {
        enabled: root.visible
        sequences: [StandardKey.Forward]
        onActivated: controller.goForward()
    }
    Shortcut {
        // Ctrl maps to Cmd on macOS: Cmd+Up there, Ctrl+Up / Alt+Up elsewhere.
        enabled: root.visible
        sequences: ["Ctrl+Up", "Alt+Up"]
        onActivated: controller.goUp()
    }
    Shortcut {
        enabled: root.visible
        sequences: ["Ctrl+Down", "Ctrl+O"]
        onActivated: controller.activate(list.currentIndex)
    }
    Shortcut {
        enabled: root.visible
        sequences: [StandardKey.Refresh]
        onActivated: controller.reload()
    }
    Shortcut {
        enabled: root.visible
        sequences: ["Ctrl+Shift+."]
        onActivated: controller.toggleHiddenFiles()
    }
    Shortcut {
        enabled: root.visible
        sequences: ["Ctrl+L"]
        onActivated: {
            locationField.forceActiveFocus()
            locationField.selectAll()
        }
    }

    // File operations (disabled while typing a name or a location)
    Shortcut {
        enabled: root.visible && list.activeFocus
        sequences: [StandardKey.SelectAll]
        onActivated: root.entries.selectAll()
    }
    Shortcut {
        enabled: root.visible && list.activeFocus
        sequences: [StandardKey.Copy]
        onActivated: root.operations.copyToClipboard(root.selection())
    }
    Shortcut {
        enabled: root.visible && list.activeFocus
        sequences: [StandardKey.Paste]
        onActivated: root.operations.paste(controller.locationId, OperationsController.Copy)
    }
    Shortcut {
        // Finder's "Move Item Here": Cmd+Option+V.
        enabled: root.visible && list.activeFocus
        sequences: ["Ctrl+Alt+V"]
        onActivated: root.operations.paste(controller.locationId, OperationsController.Move)
    }
    Shortcut {
        enabled: root.visible && list.activeFocus
        sequences: ["Ctrl+Backspace", StandardKey.Delete]
        onActivated: root.trashSelection()
    }
    Shortcut {
        enabled: root.visible && list.activeFocus
        sequences: ["Ctrl+Alt+Backspace", "Shift+Del"]
        onActivated: root.confirmPermanentDelete()
    }
    Shortcut {
        enabled: root.visible && list.activeFocus
        sequences: ["F2"]
        onActivated: root.startRename(list.currentIndex)
    }
    Shortcut {
        enabled: root.visible
        sequences: [StandardKey.Find]
        onActivated: {
            searchField.forceActiveFocus()
            searchField.selectAll()
        }
    }
    Shortcut {
        // Finder: Shift+Cmd+P shows the preview pane.
        enabled: root.visible
        sequences: ["Ctrl+Shift+P"]
        onActivated: root.previewVisible = !root.previewVisible
    }
    Shortcut {
        enabled: root.visible
        sequences: ["Ctrl+Shift+N"]
        onActivated: root.operations.newFolder(controller.locationId, root.newFolderName)
    }
}
