import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Shown when a Pro feature is used in the free edition.
Dialog {
    id: dialog

    required property LicenseController license
    property string feature: ""
    signal enterKeyRequested()

    readonly property var names: ({
        "remote-storage": qsTr("Cloud storage, S3 and servers"),
        "ai-search": qsTr("AI search"),
        "organize": qsTr("Organize"),
        "utilities": qsTr("Utilities")
    })

    function openFor(name) {
        feature = name
        open()
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(440, parent.width - 32)
    modal: true
    title: qsTr("%1 is part of Pro").arg(names[feature] ?? qsTr("This"))

    Label {
        width: dialog.availableWidth
        wrapMode: Text.Wrap
        text: qsTr("Everything for local files stays free. Pro adds cloud storage, S3 and servers, AI search and organizing, and the utilities.")
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Not Now")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }
        Button {
            text: qsTr("Enter License Key…")
            onClicked: {
                dialog.close()
                dialog.enterKeyRequested()
            }
        }
        Button {
            text: qsTr("Buy Pro…")
            highlighted: true
            visible: dialog.license.storeUrl !== ""
            onClicked: {
                Qt.openUrlExternally(dialog.license.storeUrl)
                dialog.close()
            }
        }
    }
}
