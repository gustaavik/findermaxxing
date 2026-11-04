#include "presentation/cpp/services_holder.h"

#include "presentation/src/services_bridge.cxx.h"

ServicesHolder::ServicesHolder(rust::Box<AppServices> services, QObject* parent)
    : QObject(parent)
    , m_services(std::move(services))
{
}

ServicesHolder::~ServicesHolder() = default;

const AppServices& ServicesHolder::services() const
{
    return *m_services;
}
