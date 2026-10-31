/*
  The Halloween screensaver.  Black, and every so often something happens.

  `Main.qml` shows this after `Knobs.idleSeconds` of no input and hides it again the
  moment anything is touched.  Everything here is drawn from QtQuick primitives —
  there are no photographs and no video, which is the point: nothing to license, no
  binary assets in the repo, and it scales to whatever the Pi Room monitors are.

  The only files are the sounds and the static frames, both generated from scratch by
  this theme's Nix build (see default.nix for the ffmpeg expressions), so a rebuild
  is the whole provenance.

  Pacing while idle: a scare every 7–20 s, each one lit for as long as it needs and
  then black again.  The blink durations are deliberately short on `face` and
  `static` — those are the two that are meant to make someone jump.
*/
import QtQuick 2.15
import "Knobs.js" as Knobs

Item {
    id: screen

    visible: false
    z: 50

    property string mode: "black"
    property string phrase: ""

    // Where the pointer was when the overlay appeared.  The first position change
    // after showing is kwin telling the new MouseArea it now has the pointer, not a
    // person touching the machine, and treating it as activity made every scare
    // dismiss itself within a fraction of a second.  A position of -1e9 means "no
    // baseline yet", and `shownAt` holds off movement for the first couple of
    // seconds: switching the pointer to a blank cursor (which is what keeps an arrow
    // off the black screen) makes the compositor announce the pointer again.  Keys,
    // clicks and the wheel are never ignored.
    property point anchor: Qt.point(-1e9, -1e9)
    property double shownAt: 0

    signal dismissed()

    // How long each scare is on screen before it goes black again.  The values are
    // per-scare because a face flashed for 280 ms and a face held for five seconds
    // are two different things.
    readonly property var durations: ({
            "eyes": 5200,
            "face": 300,
            "static": 430,
            "figure": 4400,
            "message": 2600,
            "red": 3800
        })

    readonly property var phrases: [
        "GO HOME",
        "TRICK OR TREAT",
        "BEHIND YOU",
        "LET ME IN",
        "IT IS STILL OCTOBER",
        "DO NOT LOOK UP"
    ]

    // The date window is month-and-day only, so it re-arms itself every year and
    // nobody has to remember to turn it off the morning after.
    function windowOpen() {
        if (Knobs.alwaysOn)
            return true;
        var now = new Date();
        var monthDay = (now.getMonth() + 1) * 100 + now.getDate();
        return monthDay >= Knobs.activeFrom && monthDay <= Knobs.activeTo;
    }

    function show() {
        if (visible)
            return;
        if (!windowOpen())
            return;
        visible = true;
        mode = "black";
        anchor = Qt.point(-1e9, -1e9);
        shownAt = Date.now();
        capture.forceActiveFocus();
        schedule.interval = 2500;
        schedule.restart();
    }

    function dismiss() {
        if (!visible)
            return;
        visible = false;
        mode = "black";
        schedule.stop();
        hold.stop();
        if (audio.status === Loader.Ready)
            audio.item.stopAll();
        dismissed();
    }

    function scare() {
        var roll = Math.random();
        var pick;
        if (roll < 0.28)
            pick = "eyes";
        else if (roll < 0.50)
            pick = "face";
        else if (roll < 0.66)
            pick = "static";
        else if (roll < 0.80)
            pick = "figure";
        else if (roll < 0.92)
            pick = "message";
        else
            pick = "red";

        phrase = phrases[Math.floor(Math.random() * phrases.length)];
        mode = pick;
        hold.interval = durations[pick];
        hold.restart();
        if (audio.status === Loader.Ready)
            audio.item.play(pick);
    }

    Timer {
        id: schedule
        repeat: false
        onTriggered: screen.scare()
    }

    Timer {
        id: hold
        repeat: false
        onTriggered: {
            screen.mode = "black";
            schedule.interval = 7000 + Math.random() * 13000;
            schedule.restart();
        }
    }

    Rectangle {
        anchors.fill: parent
        color: "#000000"
    }

    // ---- scares -------------------------------------------------------------

    // Two eyes in the dark, slowly getting closer.
    Item {
        id: eyes
        anchors.fill: parent
        visible: screen.mode === "eyes"

        Row {
            anchors.centerIn: parent
            spacing: 110
            scale: eyes.visible ? 1.3 : 0.8
            opacity: eyes.visible ? 1 : 0

            Behavior on opacity {
                NumberAnimation {
                    duration: 700
                }
            }
            Behavior on scale {
                NumberAnimation {
                    duration: 5000
                    easing.type: Easing.InQuad
                }
            }

            Repeater {
                model: 2

                Item {
                    width: 200
                    height: 200

                    Rectangle {
                        anchors.centerIn: parent
                        width: 200
                        height: 200
                        radius: 100
                        color: "#c98a1e"
                        opacity: 0.05
                    }
                    Rectangle {
                        anchors.centerIn: parent
                        width: 140
                        height: 140
                        radius: 70
                        color: "#d99b26"
                        opacity: 0.07
                    }
                    Rectangle {
                        anchors.centerIn: parent
                        width: 96
                        height: 96
                        radius: 48
                        color: "#e8b542"
                        opacity: 0.1
                    }
                    Rectangle {
                        anchors.centerIn: parent
                        width: 84
                        height: 50
                        radius: 25
                        color: "#120c04"
                        border.width: 2
                        border.color: "#e8b542"

                        Rectangle {
                            anchors.centerIn: parent
                            width: 44
                            height: 26
                            radius: 13
                            color: "#ffd069"
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            width: 11
                            height: 26
                            radius: 6
                            color: "#161006"
                        }
                    }
                }
            }
        }
    }

    // A pale face, flashed.  Big on purpose: it is on screen for 300 ms, so the
    // shape has to land before anyone has worked out what they are looking at.
    Item {
        id: face
        anchors.fill: parent
        visible: screen.mode === "face"

        Item {
            id: head
            anchors.centerIn: parent
            width: Math.min(parent.width * 0.52, parent.height * 0.62)
            height: parent.height * 0.96
            rotation: -3
            scale: face.visible ? 1.04 : 1

            Behavior on scale {
                NumberAnimation {
                    duration: 300
                }
            }

            Rectangle {
                anchors.fill: parent
                radius: width / 2
                gradient: Gradient {
                    GradientStop {
                        position: 0
                        color: "#c6bfb1"
                    }
                    GradientStop {
                        position: 0.7
                        color: "#9c9385"
                    }
                    GradientStop {
                        position: 1
                        color: "#6a6256"
                    }
                }
            }

            Rectangle {
                x: head.width * 0.10
                y: head.height * 0.23
                width: head.width * 0.32
                height: head.height * 0.29
                radius: width / 2
                color: "#0a0908"
            }
            Rectangle {
                x: head.width * 0.56
                y: head.height * 0.26
                width: head.width * 0.36
                height: head.height * 0.25
                radius: width / 2
                color: "#0a0908"
            }
            Rectangle {
                x: head.width * 0.30
                y: head.height * 0.68
                width: head.width * 0.40
                height: head.height * 0.13
                radius: width / 2
                color: "#0b0706"
            }
        }
    }

    // Something standing at the edge of the screen.  Near-black on black with one
    // lit edge, which is as much as a monitor will show and about all it needs to.
    Item {
        id: figure
        anchors.fill: parent
        visible: screen.mode === "figure"
        opacity: visible ? 1 : 0

        Behavior on opacity {
            NumberAnimation {
                duration: 1500
            }
        }

        Item {
            anchors.right: parent.right
            anchors.rightMargin: -70
            anchors.bottom: parent.bottom
            width: 380
            height: parent.height

            Rectangle {
                id: body
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                anchors.bottomMargin: -60
                width: 300
                height: 820
                radius: 150
                color: "#0d0d0d"
            }
            Rectangle {
                anchors.left: body.left
                anchors.top: body.top
                width: 7
                height: body.height * 0.75
                gradient: Gradient {
                    GradientStop {
                        position: 0
                        color: "#3a3a3a"
                    }
                    GradientStop {
                        position: 1
                        color: "transparent"
                    }
                }
            }
            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 700
                width: 180
                height: 220
                radius: 90
                color: "#111111"
                border.width: 2
                border.color: "#1f1f1f"
            }
        }
    }

    // Four frames of television static.
    Item {
        id: staticScare
        anchors.fill: parent
        visible: screen.mode === "static"

        property int frame: 0

        Timer {
            interval: 70
            repeat: true
            running: staticScare.visible
            onTriggered: staticScare.frame = (staticScare.frame + 1) % 4
        }

        Image {
            anchors.fill: parent
            source: "static/static-" + staticScare.frame + ".png"
            fillMode: Image.Stretch
            smooth: false
            cache: true
        }
    }

    // A line of text in the dark, flickering like a failing sign.
    Item {
        id: message
        anchors.fill: parent
        visible: screen.mode === "message"

        property real flick: 1

        Timer {
            interval: 90
            repeat: true
            running: message.visible
            onTriggered: message.flick = Math.random() < 0.22 ? 0.12 : 1
        }

        Text {
            anchors.centerIn: parent
            width: parent.width * 0.9
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            text: screen.phrase
            color: "#8f1414"
            opacity: message.flick
            font.pixelSize: 68
            font.letterSpacing: 14
            font.bold: true
        }
    }

    // No image at all: a red glow from below the screen and a heartbeat.  A
    // vertical gradient rather than a disc in the middle — a big rounded Rectangle
    // reads as a rounded square on a widescreen monitor, which looked like a bug.
    Item {
        id: red
        anchors.fill: parent
        visible: screen.mode === "red"
        opacity: visible ? 1 : 0

        Behavior on opacity {
            NumberAnimation {
                duration: 1100
            }
        }

        Rectangle {
            anchors.fill: parent
            gradient: Gradient {
                GradientStop {
                    position: 0
                    color: "transparent"
                }
                GradientStop {
                    position: 0.55
                    color: "#140000"
                }
                GradientStop {
                    position: 0.85
                    color: "#3d0000"
                }
                GradientStop {
                    position: 1
                    color: "#5a0000"
                }
            }
        }
    }

    // Loaded, not imported: if the greeter has no QtMultimedia (or no sound card)
    // the visual scares still work and the Loader just reports Error.
    Loader {
        id: audio
        source: "SoundPlayer.qml"
    }

    // Any key dismisses.  It swallows the key rather than letting it reach a field,
    // so a jump-scare cannot half-type someone's password.
    Item {
        id: capture
        anchors.fill: parent
        focus: true
        Keys.onPressed: screen.dismiss()
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        // The greeter's pointer is otherwise parked on the black screen all night.
        cursorShape: Qt.BlankCursor
        onClicked: screen.dismiss()
        onWheel: screen.dismiss()

        onPositionChanged: function (mouse) {
            if (Date.now() - screen.shownAt < 2500)
                return;
            if (screen.anchor.x < -1e8) {
                screen.anchor = Qt.point(mouse.x, mouse.y);
                return;
            }
            if (Math.abs(mouse.x - screen.anchor.x) + Math.abs(mouse.y - screen.anchor.y) > 20)
                screen.dismiss();
        }
    }
}
