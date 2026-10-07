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
        }

        onObjectAdded: (index, object) => object.assign(index)
    }
}
