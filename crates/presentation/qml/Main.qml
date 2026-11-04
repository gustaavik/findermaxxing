import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Window chrome and tabs only. Each tab is a FolderView with its own controller;
// the tab list itself is pure view state.
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
                text: "+"
                onClicked: window.newTab()
                ToolTip.visible: hovered
                ToolTip.text: qsTr("New tab")
                Accessible.name: qsTr("New tab")
            }
        }
    }

    StackLayout {
        anchors.fill: parent
        currentIndex: tabBar.currentIndex

        Repeater {
            id: views
            model: tabs
            delegate: FolderView {
                services: window.services
            }
        }
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
