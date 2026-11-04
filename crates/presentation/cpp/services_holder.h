#ifndef FINDERMAXXING_SERVICES_HOLDER_H
#define FINDERMAXXING_SERVICES_HOLDER_H
// Include guard instead of #pragma once: qmltyperegistrar reaches this header
// through two different paths (crate source and exported copy).

// Carries the Rust service graph through QML as an ordinary object property,
// which is how bridge objects receive their dependencies without globals.

#include <QtCore/QObject>
#include <QtQml/qqmlregistration.h>

#include "rust/cxx.h"

struct AppServices;

class ServicesHolder : public QObject
{
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("ServicesHolder is created by the application bootstrap")

public:
    explicit ServicesHolder(rust::Box<AppServices> services, QObject* parent = nullptr);
    ~ServicesHolder() override;

    const AppServices& services() const;

private:
    rust::Box<AppServices> m_services;
};

#endif // FINDERMAXXING_SERVICES_HOLDER_H
