#include "presentation/cpp/bootstrap.h"

#include <QtCore/QVariantMap>
#include <QtGui/QGuiApplication>
#include <QtQml/QQmlApplicationEngine>

#include <cstdlib>

#include "presentation/cpp/services_holder.h"
#include "presentation/src/services_bridge.cxx.h"

namespace {

constexpr auto kApplicationName = "FinderMaxxing";
constexpr auto kQmlModule = "FinderMaxxing";
constexpr auto kMainComponent = "Main";
constexpr auto kServicesProperty = "services";

} // namespace

std::int32_t runApplication(rust::Box<AppServices> services)
{
    // QGuiApplication keeps references to argc/argv for its whole lifetime,
    // so both live in this frame, which outlives the application object.
    int argc = 1;
    char programName[] = "FinderMaxxing";
    char* argv[] = { programName, nullptr };

    QGuiApplication application(argc, argv);
    QGuiApplication::setApplicationName(kApplicationName);
    QGuiApplication::setApplicationDisplayName(kApplicationName);

    // Declared before the engine so it is destroyed after every QML object.
    ServicesHolder holder(std::move(services));

    QQmlApplicationEngine engine;
    QObject::connect(
        &engine, &QQmlApplicationEngine::objectCreationFailed, &application,
        [] { QCoreApplication::exit(EXIT_FAILURE); }, Qt::QueuedConnection);
    engine.setInitialProperties({ { kServicesProperty, QVariant::fromValue(&holder) } });
    engine.loadFromModule(kQmlModule, kMainComponent);

    return application.exec();
}
