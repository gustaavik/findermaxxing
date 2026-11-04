import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Favourites, places, volumes and cloud accounts in one list. Opening and
// managing entries goes through SidebarController; this file only lays out.
Pane {
    id: sidebar

    required property SidebarController controller

    readonly property int rowHeight: 26
    readonly property int indent: 12
    // Plain glyphs until native icons arrive with icon themes.
    readonly property var glyphs: ({ home: "⌂", folder: "▤", drive: "▭", cloud: "☁", bucket: "◫" })

    signal openRequested(string location)

    padding: 0

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: sidebar.controller.model
            Accessible.role: Accessible.List
            Accessible.name: qsTr("Sidebar")
            ScrollBar.vertical: ScrollBar {}

            delegate: ItemDelegate {
                id: entry

                required property int kind
                required property string label
                required property string detail
                required property string location
                required property string iconName
                required property var accountId

                readonly property bool isHeading: kind === SidebarController.Heading
                readonly property bool isAccount: kind === SidebarController.Account
                readonly property bool openable: location !== ""

                width: ListView.view.width
                height: isHeading ? sidebar.rowHeight * 1.2 : (detail !== "" ? sidebar.rowHeight * 1.6 : sidebar.rowHeight)
                enabled: !isHeading
                leftPadding: isHeading || isAccount ? sidebar.indent : sidebar.indent * 2
                Accessible.name: detail !== "" ? label + ", " + detail : label

                contentItem: RowLayout {
                    spacing: 6
                    Label {
                        visible: !entry.isHeading
                        text: sidebar.glyphs[entry.iconName] ?? ""
                        color: palette.placeholderText
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        Label {
                            Layout.fillWidth: true
                            text: entry.isHeading ? entry.label.toUpperCase() : entry.label
                            font.bold: entry.isHeading || entry.isAccount
                            font.pointSize: entry.isHeading ? Qt.application.font.pointSize * 0.8 : Qt.application.font.pointSize
                            color: entry.isHeading ? palette.placeholderText : palette.text
                            elide: Text.ElideRight
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: entry.detail !== ""
                            text: entry.detail
                            font.pointSize: Qt.application.font.pointSize * 0.8
                            color: palette.placeholderText
                            elide: Text.ElideRight
                        }
                    }
                }

                onClicked: if (openable) sidebar.openRequested(location)

                TapHandler {
                    acceptedButtons: Qt.RightButton
                    onTapped: {
                        if (entry.kind === SidebarController.Favorite || entry.isAccount) {
                            itemMenu.target = entry
                            itemMenu.popup()
                        }
                    }
                }
            }
        }

        Button {
            Layout.fillWidth: true
            Layout.margins: 8
            text: qsTr("Add Cloud Storage…")
            onClicked: addAccount.open()
        }
    }

    Menu {
        id: itemMenu

        property var target: null

        popupType: Popup.Native

        MenuItem {
            visible: itemMenu.target !== null && itemMenu.target.kind === SidebarController.Favorite
            height: visible ? implicitHeight : 0
            text: qsTr("Remove from Sidebar")
            onTriggered: sidebar.controller.removeFavorite(itemMenu.target.location)
        }
        MenuItem {
            visible: itemMenu.target !== null && itemMenu.target.isAccount
            height: visible ? implicitHeight : 0
            text: qsTr("Add Bucket by Name…")
            onTriggered: {
                addBucket.accountId = itemMenu.target.accountId
                addBucket.open()
            }
        }
        MenuItem {
            visible: itemMenu.target !== null && itemMenu.target.isAccount
            height: visible ? implicitHeight : 0
            text: qsTr("Remove Account…")
            onTriggered: {
                removeAccount.accountId = itemMenu.target.accountId
                removeAccount.accountName = itemMenu.target.label
                removeAccount.open()
            }
        }
    }

    Dialog {
        id: removeAccount

        property var accountId: 0
        property string accountName: ""

        parent: Overlay.overlay
        anchors.centerIn: parent
        modal: true
        title: qsTr("Remove “%1”?").arg(accountName)
        standardButtons: Dialog.Cancel | Dialog.Ok
        onAccepted: sidebar.controller.removeAccount(accountId)

        Label {
            text: qsTr("The account and its saved secret are removed from this computer.\nNothing in the buckets is deleted.")
        }
    }

    Dialog {
        id: addBucket

        property var accountId: 0

        parent: Overlay.overlay
        anchors.centerIn: parent
        modal: true
        title: qsTr("Add Bucket")
        standardButtons: Dialog.Cancel | Dialog.Ok
        onAboutToShow: bucketName.text = ""
        onAccepted: sidebar.controller.addBucket(accountId, bucketName.text)

        ColumnLayout {
            Label {
                text: qsTr("For keys that can open a bucket but not list all buckets.")
                color: palette.placeholderText
            }
            TextField {
                id: bucketName
                Layout.fillWidth: true
                placeholderText: qsTr("bucket-name")
                Accessible.name: qsTr("Bucket name")
                onAccepted: addBucket.accept()
            }
        }
    }

    AddAccountDialog {
        id: addAccount
        controller: sidebar.controller
    }

    Component.onCompleted: controller.refresh()
}
