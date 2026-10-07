import QtQuick
import dev.falconshot 1.0

// The app's root. It has no scene of its own on purpose: what this process shows
// is either a pin window or a capture mask, both of which are full citizens of the
// window list and neither of which wants a parent. §5.1's "no main window, a tray
// icon" ends up looking exactly like this - a headful engine whose root object is
// invisible.
Item {
    id: app

    property PinShim shim: PinShim {}
    property Session session: Session {}

    /// The mask windows, indexed by Qt screen index. Kept by the two signals rather
    /// than read back: this Qt's `Instantiator` has `count`, `object` (singular) and
    /// `delegateModelAccess`, and no `objects` list - measured against 6.10.1's own
    /// `qml/QtQml/Models/plugins.qmltypes`, because a wrong property name inside a
    /// function body is a JS `TypeError` that on Windows never reaches stdout (§8-R14).
    property var maskWindows: []

    Component.onCompleted: {
        app.session.markLoaded()
        app.session.refresh()
        app.session.refreshMasks()
    }

    // One window per pin, in z order.
    //
    // `Instantiator`, not `Repeater`: a Repeater parents what it makes into the
    // scene, and a Window is not scene content. Measured - a Repeater over the
    // same model and the same delegate left `topLevelWindows=0`.
    //
    // Membership changes are the only thing that has to come through
    // `session.count`; everything a pin does to itself stays inside its own
    // window and its own PinView.
    Instantiator {
        id: pins
        model: app.session.count

        delegate: PinWindow {
            shim: app.shim
            session: app.session
        }

        // Neither `index` nor `modelData` exists inside a QQmlInstantiator
        // delegate - measured on 6.10.1 - so the slot comes from the signal.
        // `assign` rather than a property write: the window owns the PinView, and
        // the two have to be pointed at the same pin before the first frame.
        onObjectAdded: (index, object) => object.assign(app.session.pinId(index))
    }

    // One mask per screen, while a capture flow is running.
    //
    // A second `Instantiator` rather than a shared delegate: a pin's model is the
    // set of pictures the user owns and a mask's model is the list of screens, and
    // the two change for entirely unrelated reasons. `index` is the Qt screen
    // index, which is what the mask state matched its frozen frame against.
    Instantiator {
        id: masks
        model: app.session.mask_count

        delegate: CaptureMask {
            shim: app.shim
            session: app.session
            onGrabbed: app.refreshOtherMasks(this)
        }

        onObjectAdded: (index, object) => {
            object.assign(index)
            app.maskWindows[index] = object
        }

        // `endMask` comes through here: the model is the slot count, so closing the
        // overlay is what retires the windows.
        onObjectRemoved: (index) => { app.maskWindows[index] = null }
    }

    /// One selection, in desktop pixels; N windows, each drawing its own clipped
    /// piece of it. The window that took the pointer event has already reloaded
    /// itself, so this is the other one - told by the signal rather than by a timer,
    /// because a dim that catches up a frame late is a dim the user watches lag the
    /// mouse across the seam.
    ///
    /// Reload-only: it never re-emits `grabbed`, which is what stops two windows
    /// from refreshing each other forever.
    function refreshOtherMasks(from) {
        for (let i = 0; i < maskWindows.length; ++i) {
            const m = maskWindows[i]
            if (m && m !== from) {
                m.view.reload()
            }
        }
    }
}
