import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Window chrome, tabs, and the window-wide file-operation UI (jobs, conflicts,
// errors, undo/redo). Each tab is a FolderView with its own controller; the
// tab list itself is pure view state.
ApplicationWindow {
    id: window

    required property ServicesHolder services

    readonly property int initialWidth: 1100
    readonly property int initialHeight: 720
    readonly property var currentView: views.itemAt(tabBar.currentIndex)

    width: initialWidth
    height: initialHeight
    visible: true
    title: currentView ? currentView.controller.title : Qt.application.displayName

    function newTab() {
        tabs.append({ opened: Date.now() })
        tabBar.currentIndex = tabs.count - 1
    }

    function toggleTerminal() {
        terminal.visible = !terminal.visible
        if (terminal.visible) {
            terminal.start()
        }
    }

    function closeTab(index) {
        if (tabs.count === 1) {
            window.close()
            return
        }
        tabs.remove(index)
    }

    OperationsController {
        id: fileOps
        services: window.services
        // Folders that can't be watched (S3) reload when a job changes them,
        // and the content index looks for what changed.
        onFoldersChanged: locations => {
            if (aiSearch.enabled) {
                aiSearch.rescan()
            }
            for (let i = 0; i < views.count; ++i) {
                views.itemAt(i).controller.reloadIfShowing(locations)
            }
        }
    }

    SidebarController {
        id: sidebarController
        services: window.services
    }

    AiSearchController {
        id: aiSearch
        services: window.services
        Component.onCompleted: refresh()
    }

    // `opened` only exists because a ListModel needs at least one role.
    ListModel {
        id: tabs
        ListElement { opened: 0 }
    }

    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            spacing: 0

            TabBar {
                id: tabBar
                Layout.fillWidth: true
                Accessible.name: qsTr("Tabs")

                Repeater {
                    model: tabs
                    delegate: TabButton {
                        required property int index
                        readonly property var view: views.itemAt(index)
                        text: view ? view.controller.title : ""
                        width: Math.max(implicitWidth, 140)
                        Accessible.name: qsTr("Tab %1").arg(text)
                    }
                }
            }

            ToolButton {
                text: "↶"
                enabled: fileOps.canUndo
                onClicked: fileOps.undo()
                ToolTip.visible: hovered
                ToolTip.text: fileOps.undoText
                Accessible.name: fileOps.canUndo ? fileOps.undoText : qsTr("Undo")
            }
            ToolButton {
                text: "↷"
                enabled: fileOps.canRedo
                onClicked: fileOps.redo()
                ToolTip.visible: hovered
                ToolTip.text: fileOps.redoText
                Accessible.name: fileOps.canRedo ? fileOps.redoText : qsTr("Redo")
            }
            ToolButton {
                text: aiSearch.working ? "✦ " + Math.round(Math.max(0, aiSearch.progress) * 100) + "%" : "✦"
                onClicked: aiDialog.open()
                ToolTip.visible: hovered
                ToolTip.text: aiSearch.enabled ? aiSearch.statusText : qsTr("AI search is off")
                Accessible.name: qsTr("AI search: %1").arg(aiSearch.enabled ? aiSearch.statusText : qsTr("off"))
            }
            ToolButton {
                text: ">_"
                checkable: true
                checked: terminal.visible
                onClicked: window.toggleTerminal()
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Terminal")
                Accessible.name: qsTr("Show or hide the terminal")
            }
            ToolButton {
                text: "+"
                onClicked: window.newTab()
                ToolTip.visible: hovered
                ToolTip.text: qsTr("New tab")
                Accessible.name: qsTr("New tab")
            }
        }
    }

    SplitView {
        anchors.fill: parent
        orientation: Qt.Horizontal

    Sidebar {
        SplitView.preferredWidth: 210
        SplitView.minimumWidth: 140
        controller: sidebarController
        operations: fileOps
        onOpenRequested: location => window.currentView.controller.openLocationId(location)
    }

    ColumnLayout {
        SplitView.fillWidth: true
        spacing: 0

        SplitView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            orientation: Qt.Vertical

            StackLayout {
                SplitView.fillHeight: true
                currentIndex: tabBar.currentIndex

                Repeater {
                    id: views
                    model: tabs
                    delegate: FolderView {
                        services: window.services
                        operations: fileOps
                        sidebar: sidebarController
                        onFindDuplicatesRequested: (location, label) => duplicates.openFor(location, label)
                        onDiskUsageRequested: location => diskUsage.openFor(location)
                    }
                }
            }

            TerminalPane {
                id: terminal
                visible: false
                SplitView.preferredHeight: 240
                SplitView.minimumHeight: 80
                services: window.services
                folder: window.currentView ? window.currentView.controller.locationId : ""
                onCloseRequested: visible = false
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: errorRow.implicitHeight + 12
            visible: fileOps.errorMessage !== ""
            color: palette.alternateBase

            RowLayout {
                id: errorRow
                anchors.fill: parent
                anchors.margins: 6
                anchors.leftMargin: 12

                Label {
                    Layout.fillWidth: true
                    text: fileOps.errorMessage
                    wrapMode: Text.Wrap
                    Accessible.role: Accessible.AlertMessage
                    Accessible.name: text
                }
                ToolButton {
                    text: "✕"
                    onClicked: fileOps.dismissError()
                    Accessible.name: qsTr("Dismiss")
                }
            }
        }

        JobsPanel {
            Layout.fillWidth: true
            operations: fileOps
        }
    }
    }

    CostDialog {
        operations: fileOps
    }

    DuplicatesWindow {
        id: duplicates
        services: window.services
        operations: fileOps
    }

    DiskUsageWindow {
        id: diskUsage
        services: window.services
        operations: fileOps
    }

    AiSearchDialog {
        id: aiDialog
        controller: aiSearch
    }

    // Copies and moves a quit or crash cut short, offered once at start-up.
    Dialog {
        id: interrupted

        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(480, parent.width - 32)
        modal: true
        visible: fileOps.interruptedCount > 0
        closePolicy: Popup.NoAutoClose
        title: fileOps.interruptedCount === 1 ? qsTr("A transfer didn't finish")
                                              : qsTr("%1 transfers didn't finish").arg(fileOps.interruptedCount)

        ColumnLayout {
            anchors.fill: parent
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: qsTr("%1.\n\nResume continues where they stopped; what was already transferred is kept. Discard deletes the unfinished parts.").arg(fileOps.interruptedText)
            }
        }

        footer: DialogButtonBox {
            Button {
                text: qsTr("Discard")
                onClicked: fileOps.discardInterrupted()
            }
            Button {
                text: qsTr("Resume")
                highlighted: true
                onClicked: fileOps.resumeInterrupted()
            }
        }
    }

    Component.onCompleted: fileOps.checkInterrupted()

    ConflictDialog {
        operations: fileOps
    }

    Shortcut {
        sequences: [StandardKey.Undo]
        enabled: fileOps.canUndo
        onActivated: fileOps.undo()
    }
    Shortcut {
        sequences: [StandardKey.Redo]
        enabled: fileOps.canRedo
        onActivated: fileOps.redo()
    }

    Shortcut {
        // Ctrl+` as in VS Code; Qt calls macOS's ⌃ key "Meta".
        sequence: Qt.platform.os === "osx" ? "Meta+`" : "Ctrl+`"
        context: Qt.ApplicationShortcut
        onActivated: window.toggleTerminal()
    }
    Shortcut {
        sequences: [StandardKey.AddTab]
        onActivated: window.newTab()
    }
    Shortcut {
        sequences: [StandardKey.Close]
        onActivated: window.closeTab(tabBar.currentIndex)
    }
    Shortcut {
        sequences: [StandardKey.NextChild]
        onActivated: tabBar.currentIndex = (tabBar.currentIndex + 1) % tabs.count
    }
    Shortcut {
        sequences: [StandardKey.PreviousChild]
        onActivated: tabBar.currentIndex = (tabBar.currentIndex + tabs.count - 1) % tabs.count
    }
}
