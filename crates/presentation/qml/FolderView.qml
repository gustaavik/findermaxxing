import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// One folder listing. Layout and input routing only; every decision is made
// by FolderViewController.
FocusScope {
    id: root

    required property ServicesHolder services
    readonly property alias controller: controller

    readonly property int rowHeight: 24
    readonly property int iconSize: 16
    readonly property int sizeColumnWidth: 90
    readonly property int modifiedColumnWidth: 170
    readonly property int columnSpacing: 12
    readonly property int sidePadding: 10

    focus: true

    FolderViewController {
        id: controller
        services: root.services
        Component.onCompleted: openHome()
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
            }
        }

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

            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            focus: true
            reuseItems: true
            boundsBehavior: Flickable.StopAtBounds
            keyNavigationEnabled: true
            highlightMoveDuration: 0
            model: controller.model
            Accessible.role: Accessible.List
            Accessible.name: qsTr("Contents of %1").arg(controller.title)

            ScrollBar.vertical: ScrollBar {}

            delegate: ItemDelegate {
                id: row

                required property int index
                required property string name
                required property string iconName
                required property string sizeText
                required property var modified
                required property bool isContainer
                required property bool isHidden

                width: ListView.view.width
                height: root.rowHeight
                padding: 0
                leftPadding: root.sidePadding
                rightPadding: root.sidePadding
                highlighted: ListView.isCurrentItem
                opacity: isHidden ? 0.6 : 1.0

                Accessible.role: Accessible.ListItem
                Accessible.name: name

                contentItem: RowLayout {
                    spacing: root.columnSpacing

                    FileIcon {
                        Layout.preferredWidth: root.iconSize
                        Layout.preferredHeight: root.iconSize
                        kind: row.iconName
                    }
                    Label {
                        Layout.fillWidth: true
                        text: row.name
                        elide: Text.ElideMiddle
                        color: row.highlighted ? palette.highlightedText : palette.text
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

                onClicked: {
                    list.currentIndex = index
                    list.forceActiveFocus()
                }
                onDoubleClicked: controller.activate(index)
            }

            Keys.onReturnPressed: controller.activate(currentIndex)
            Keys.onEnterPressed: controller.activate(currentIndex)

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
                    text: qsTr("%n item(s)", "", controller.itemCount)
                    Accessible.name: text
                }
            }
        }
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
        sequences: ["Ctrl+Down"]
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
}
