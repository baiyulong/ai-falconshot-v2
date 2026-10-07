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
        app.session.refreshWarm()
    }

    // The `--mask` gauge's second and later rounds, which is the only way this
    // process can measure what a hot key costs: by then Qt, the engine, the compiled
    // document and one presented frame are all paid for, and none of that is a
    // per-capture cost. Rust owns the arithmetic and the count; this Timer exists
    // because nothing else here can act after the event loop starts.
    //
    // `running` binds to a qproperty rather than calling an invokable in a loop:
    // `warm_left` notifies as it counts down, so the timer stops itself, and a
    // handler that re-armed it would run rounds the plan never asked for.
    //
    // The two names are not spelled the same way, and this is measured rather than
    // assumed: cxx-qt keeps a qproperty's Rust identifier (`warm_left`,
    // `warm_step`, `mask_count` - read out of the generated moc table) and camelCases
    // an invokable (`refreshWarm`, `warmRound`). An undefined property inside a
    // binding is only a warning and leaves the binding at 0, which is why the first
    // run of this timer loaded the document and then did nothing at all.
    Timer {
        id: warmRounds
        interval: app.session.warm_step
        repeat: true
        running: app.session.warm_left > 0
        // `onTriggered`, the signal handler - `triggered` is not a property, and
        // assigning to a non-existent one is a document error that loses the whole
        // file, not just this timer (§8-R14).
        onTriggered: {
            app.session.warmRound()
            // Same screens, so no window was rebuilt - they have to be told the
            // frame key changed, or they keep showing the round before this one.
            app.refreshOtherMasks(null)
        }
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
