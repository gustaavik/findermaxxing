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
        // Folders that can't be watched (S3) reload when a job changes them.
        onFoldersChanged: locations => {
            for (let i = 0; i < views.count; ++i) {
                views.itemAt(i).controller.reloadIfShowing(locations)
            }
        }
    }

    SidebarController {
        id: sidebarController
        services: window.services
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
        onOpenRequested: location => window.currentView.controller.openLocationId(location)
    }

    ColumnLayout {
        SplitView.fillWidth: true
        spacing: 0

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabBar.currentIndex

            Repeater {
                id: views
                model: tabs
                delegate: FolderView {
                    services: window.services
                    operations: fileOps
                    sidebar: sidebarController
                }
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
