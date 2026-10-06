import QtQuick
import dev.falconshot 1.0

// The app's root. It has no scene of its own on purpose: the only visual things
// this process makes are pin windows. §5.1's "no main window, a tray icon" ends
// up looking exactly like this - a headful engine whose root object is invisible.
Item {
    id: app

    property PinShim shim: PinShim {}
    property Session session: Session {}

    Component.onCompleted: {
        app.session.markLoaded()
        app.session.refresh()
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
}
