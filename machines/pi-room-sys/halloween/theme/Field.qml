/*
  One text field for the Pi Room greeter: a dark box with a placeholder and a
  dim-red border once it has focus.  Plain QtQuick rather than SddmComponents'
  TextBox, because that one hard-codes its colours and its own focus handling.

  It reports `activity` on anything that means the person is still there — a
  keystroke, a paste, a click — which is what keeps the Halloween screensaver from
  appearing over someone halfway through typing a password.
*/
import QtQuick 2.15

Item {
    id: field

    property string placeholder: ""
    property alias text: input.text
    property alias echoMode: input.echoMode
    property Item nextField: null
    readonly property Item textInput: input

    signal activity()
    signal accepted()

    implicitWidth: 360
    implicitHeight: 46

    function forceFocus() {
        input.forceActiveFocus();
    }

    function clear() {
        input.text = "";
    }

    Rectangle {
        anchors.fill: parent
        radius: 2
        color: input.activeFocus ? "#141414" : "#101010"
        border.width: 1
        border.color: input.activeFocus ? "#7a1f1f" : "#333333"

        Behavior on border.color {
            ColorAnimation {
                duration: 120
            }
        }

        Text {
            anchors.verticalCenter: parent.verticalCenter
            anchors.left: parent.left
            anchors.leftMargin: 14
            color: "#6b6b6b"
            font.pixelSize: 15
            text: field.placeholder
            visible: input.text.length === 0
        }

        TextInput {
            id: input
            anchors.fill: parent
            anchors.leftMargin: 14
            anchors.rightMargin: 14
            verticalAlignment: TextInput.AlignVCenter
            color: "#e2e2e2"
            selectionColor: "#7a1f1f"
            selectedTextColor: "#ffffff"
            font.pixelSize: 16
            passwordCharacter: "\u2022"
            clip: true

            KeyNavigation.tab: field.nextField ? field.nextField.textInput : null

            onTextChanged: field.activity()
            onAccepted: field.accepted()

            // Non-text keys (arrows, backspace on an empty field) change no text.
            Keys.onPressed: field.activity()
        }
    }
}
