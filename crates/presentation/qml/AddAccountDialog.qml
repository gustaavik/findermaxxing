import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Connects an S3 or S3-compatible account. The account is only saved once it
// lists its buckets; secrets go to the system keychain.
Dialog {
    id: dialog

    required property SidebarController controller

    readonly property int preferredWidth: 520
    // Endpoint hints and URL style per provider; the user can edit everything.
    readonly property var presets: [
        { name: "Amazon S3", endpoint: "", region: "us-east-1", pathStyle: false },
        { name: "MinIO / SeaweedFS (self-hosted)", endpoint: "http://localhost:9000", region: "us-east-1", pathStyle: true },
        { name: "Cloudflare R2", endpoint: "https://<account-id>.r2.cloudflarestorage.com", region: "auto", pathStyle: false },
        { name: "Backblaze B2", endpoint: "https://s3.<region>.backblazeb2.com", region: "us-west-004", pathStyle: false },
        { name: "Wasabi", endpoint: "https://s3.<region>.wasabisys.com", region: "us-east-1", pathStyle: false }
    ]

    function applyPreset(index) {
        const preset = presets[index]
        nameField.text = preset.name
        endpointField.text = preset.endpoint
        regionField.text = preset.region
        pathStyle.checked = preset.pathStyle
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(preferredWidth, parent.width - 32)
    modal: true
    title: qsTr("Add Cloud Storage")
    onAboutToShow: applyPreset(provider.currentIndex)

    Connections {
        target: dialog.controller
        function onAccountAdded() {
            secretField.text = ""
            dialog.close()
        }
    }

    GridLayout {
        anchors.fill: parent
        columns: 2

        Label { text: qsTr("Provider:") }
        ComboBox {
            id: provider
            Layout.fillWidth: true
            model: dialog.presets.map(p => p.name)
            onActivated: index => dialog.applyPreset(index)
        }
        Label { text: qsTr("Name:") }
        TextField { id: nameField; Layout.fillWidth: true; Accessible.name: qsTr("Account name") }
        Label { text: qsTr("Endpoint:") }
        TextField {
            id: endpointField
            Layout.fillWidth: true
            placeholderText: qsTr("Leave empty for Amazon S3")
            Accessible.name: qsTr("Endpoint URL")
        }
        Label { text: qsTr("Region:") }
        TextField { id: regionField; Layout.fillWidth: true; Accessible.name: qsTr("Region") }
        Label { text: qsTr("Sign in with:") }
        ComboBox {
            id: auth
            Layout.fillWidth: true
            textRole: "text"
            valueRole: "value"
            model: [
                { text: qsTr("Access key"), value: SidebarController.AccessKey },
                { text: qsTr("AWS profile (~/.aws, SSO)"), value: SidebarController.Profile },
                { text: qsTr("Default AWS credentials"), value: SidebarController.DefaultChain }
            ]
        }
        Label {
            visible: auth.currentValue !== SidebarController.DefaultChain
            text: auth.currentValue === SidebarController.Profile ? qsTr("Profile:") : qsTr("Access key ID:")
        }
        TextField {
            id: identityField
            Layout.fillWidth: true
            visible: auth.currentValue !== SidebarController.DefaultChain
            Accessible.name: auth.currentValue === SidebarController.Profile ? qsTr("Profile name") : qsTr("Access key ID")
        }
        Label { visible: auth.currentValue === SidebarController.AccessKey; text: qsTr("Secret key:") }
        TextField {
            id: secretField
            Layout.fillWidth: true
            visible: auth.currentValue === SidebarController.AccessKey
            echoMode: TextInput.Password
            Accessible.name: qsTr("Secret access key")
        }
        Item { width: 1 }
        CheckBox { id: pathStyle; text: qsTr("Path-style URLs (needed by most self-hosted servers)") }
        Item { width: 1 }
        Label {
            Layout.fillWidth: true
            text: qsTr("The secret is stored in the system keychain, never in FinderMaxxing's files.")
            wrapMode: Text.Wrap
            color: palette.placeholderText
        }
        Item { width: 1 }
        Label {
            Layout.fillWidth: true
            visible: dialog.controller.accountError !== ""
            text: dialog.controller.accountError
            wrapMode: Text.Wrap
            color: "#d9534f"
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Cancel")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }
        Button {
            text: dialog.controller.addingAccount ? qsTr("Connecting…") : qsTr("Add")
            highlighted: true
            enabled: !dialog.controller.addingAccount && nameField.text.trim() !== ""
            onClicked: dialog.controller.addAccount(
                           [nameField.text, endpointField.text, regionField.text, identityField.text, secretField.text],
                           auth.currentValue,
                           pathStyle.checked ? SidebarController.PathStyle : SidebarController.VirtualHosted)
        }
    }
}
