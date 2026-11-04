import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Connects an SFTP, WebDAV or SMB server. The server is only saved once
// signing in works; passwords and key passphrases go to the system keychain.
Dialog {
    id: dialog

    required property SidebarController controller

    readonly property int preferredWidth: 520
    // Per protocol: what the folder field means and which sign-ins exist.
    readonly property var protocols: [
        { name: "SFTP (SSH)", kind: SidebarController.Sftp, port: "22",
          folderLabel: qsTr("Folder:"), folderHint: qsTr("optional, e.g. /srv/media"),
          signIns: [SidebarController.Password, SidebarController.Agent, SidebarController.KeyFile] },
        { name: "WebDAV", kind: SidebarController.WebDav, port: "443",
          folderLabel: qsTr("Path:"), folderHint: qsTr("e.g. remote.php/dav/files/you"),
          signIns: [SidebarController.Password, SidebarController.Guest] },
        { name: "SMB (Windows, NAS)", kind: SidebarController.Smb, port: "445",
          folderLabel: qsTr("Share:"), folderHint: qsTr("share, or share/folder"),
          signIns: [SidebarController.Password, SidebarController.Guest] }
    ]
    readonly property var signInNames: ({
        [SidebarController.Password]: qsTr("Password"),
        [SidebarController.Agent]: qsTr("SSH agent"),
        [SidebarController.KeyFile]: qsTr("Key file"),
        [SidebarController.Guest]: qsTr("Guest / none")
    })
    readonly property var current: protocols[protocol.currentIndex]
    readonly property int signIn: current.signIns[Math.min(signInBox.currentIndex, current.signIns.length - 1)]
    readonly property bool usesSecret: signIn === SidebarController.Password || signIn === SidebarController.KeyFile

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(preferredWidth, parent.width - 32)
    modal: true
    title: qsTr("Connect to Server")
    onAboutToShow: secretField.text = ""

    Connections {
        target: dialog.controller
        function onServerAdded() {
            secretField.text = ""
            dialog.close()
        }
    }

    GridLayout {
        anchors.fill: parent
        columns: 2

        Label { text: qsTr("Protocol:") }
        ComboBox {
            id: protocol
            Layout.fillWidth: true
            model: dialog.protocols.map(p => p.name)
            onActivated: signInBox.currentIndex = 0
            Accessible.name: qsTr("Protocol")
        }
        Label { text: qsTr("Server:") }
        RowLayout {
            Layout.fillWidth: true
            TextField {
                id: hostField
                Layout.fillWidth: true
                placeholderText: qsTr("nas.local or 192.168.1.10")
                Accessible.name: qsTr("Server address")
            }
            TextField {
                id: portField
                Layout.preferredWidth: 70
                placeholderText: dialog.current.port
                validator: IntValidator { bottom: 1; top: 65535 }
                Accessible.name: qsTr("Port")
            }
        }
        Label { text: dialog.current.folderLabel }
        TextField {
            id: folderField
            Layout.fillWidth: true
            placeholderText: dialog.current.folderHint
            Accessible.name: qsTr("Folder or share")
        }
        Label { text: qsTr("Name:") }
        TextField {
            id: nameField
            Layout.fillWidth: true
            placeholderText: hostField.text
            Accessible.name: qsTr("Name in the sidebar")
        }
        Label { text: qsTr("Sign in with:") }
        ComboBox {
            id: signInBox
            Layout.fillWidth: true
            model: dialog.current.signIns.map(s => dialog.signInNames[s])
            Accessible.name: qsTr("Sign-in method")
        }
        Label {
            visible: dialog.signIn !== SidebarController.Guest
            text: qsTr("User name:")
        }
        TextField {
            id: userField
            Layout.fillWidth: true
            visible: dialog.signIn !== SidebarController.Guest
            Accessible.name: qsTr("User name")
        }
        Label {
            visible: dialog.signIn === SidebarController.KeyFile
            text: qsTr("Key file:")
        }
        TextField {
            id: keyField
            Layout.fillWidth: true
            visible: dialog.signIn === SidebarController.KeyFile
            placeholderText: "~/.ssh/id_ed25519"
            Accessible.name: qsTr("Private key file")
        }
        Label {
            visible: dialog.usesSecret
            text: dialog.signIn === SidebarController.KeyFile ? qsTr("Passphrase:") : qsTr("Password:")
        }
        TextField {
            id: secretField
            Layout.fillWidth: true
            visible: dialog.usesSecret
            echoMode: TextInput.Password
            placeholderText: dialog.signIn === SidebarController.KeyFile ? qsTr("if the key has one") : ""
            Accessible.name: dialog.signIn === SidebarController.KeyFile ? qsTr("Key passphrase") : qsTr("Password")
        }
        Item { width: 1; visible: dialog.current.kind === SidebarController.WebDav }
        CheckBox {
            id: encrypted
            visible: dialog.current.kind === SidebarController.WebDav
            checked: true
            text: qsTr("Use HTTPS")
        }
        Item { width: 1 }
        Label {
            Layout.fillWidth: true
            visible: dialog.current.kind === SidebarController.WebDav && !encrypted.checked
            text: qsTr("Without HTTPS, your password and files cross the network unencrypted.")
            wrapMode: Text.Wrap
            color: "#d9534f"
        }
        Item { width: 1 }
        Label {
            Layout.fillWidth: true
            text: qsTr("Passwords are stored in the system keychain, never in FinderMaxxing's files.")
            wrapMode: Text.Wrap
            color: palette.placeholderText
        }
        Item { width: 1 }
        Label {
            Layout.fillWidth: true
            visible: dialog.controller.serverError !== ""
            text: dialog.controller.serverError
            wrapMode: Text.Wrap
            color: "#d9534f"
            Accessible.role: Accessible.AlertMessage
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Cancel")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }
        Button {
            text: dialog.controller.addingServer ? qsTr("Connecting…") : qsTr("Connect")
            highlighted: true
            enabled: !dialog.controller.addingServer && hostField.text.trim() !== ""
            onClicked: dialog.controller.addServer(
                           [nameField.text, hostField.text, portField.text,
                            dialog.signIn === SidebarController.Guest ? "" : userField.text,
                            folderField.text, dialog.usesSecret ? secretField.text : "", keyField.text],
                           dialog.current.kind,
                           dialog.signIn,
                           encrypted.checked || dialog.current.kind !== SidebarController.WebDav
                               ? SidebarController.Encrypted : SidebarController.Unencrypted)
        }
    }
}
