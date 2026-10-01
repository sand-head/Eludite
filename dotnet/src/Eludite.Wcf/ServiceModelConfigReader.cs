using System.Xml.Linq;

namespace Eludite.Wcf;

/// <summary>Reads WCF endpoints out of an <c>app.config</c> or <c>web.config</c>.</summary>
public static class ServiceModelConfigReader
{
    /// <summary>
    /// Returns the endpoints under <c>configuration/system.serviceModel/client</c> and
    /// <c>configuration/system.serviceModel/services/service</c>, in document order.
    /// Elements are matched by local name so a default configuration namespace does not matter.
    /// </summary>
    /// <exception cref="System.Xml.XmlException">The text is not well-formed XML.</exception>
    public static IReadOnlyList<ServiceModelEndpoint> ReadEndpoints(string configXml)
    {
        ArgumentNullException.ThrowIfNull(configXml);

        var root = XDocument.Parse(configXml).Root;
        if (root is null || root.Name.LocalName != "configuration")
        {
            return [];
        }

        var endpoints = new List<ServiceModelEndpoint>();
        foreach (var serviceModel in Children(root, "system.serviceModel"))
        {
            foreach (var client in Children(serviceModel, "client"))
            {
                endpoints.AddRange(Children(client, "endpoint").Select(e => ToEndpoint(e, EndpointSource.Client, null)));
            }

            foreach (var service in Children(serviceModel, "services").SelectMany(s => Children(s, "service")))
            {
                var serviceName = (string?)service.Attribute("name");
                endpoints.AddRange(Children(service, "endpoint").Select(e => ToEndpoint(e, EndpointSource.Service, serviceName)));
            }
        }

        return endpoints;
    }

    private static IEnumerable<XElement> Children(XElement parent, string localName) =>
        parent.Elements().Where(e => e.Name.LocalName == localName);

    private static ServiceModelEndpoint ToEndpoint(XElement endpoint, EndpointSource source, string? serviceName) =>
        new(
            source,
            serviceName,
            (string?)endpoint.Attribute("name"),
            (string?)endpoint.Attribute("address"),
            (string?)endpoint.Attribute("binding"),
            (string?)endpoint.Attribute("contract"));
}
