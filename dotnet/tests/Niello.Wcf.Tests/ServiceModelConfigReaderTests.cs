namespace Niello.Wcf.Tests;

public sealed class ServiceModelConfigReaderTests
{
    [Fact]
    public void ReadEndpoints_ReadsClientEndpointsFromAppConfig()
    {
        const string config = """
            <?xml version="1.0" encoding="utf-8"?>
            <configuration>
              <startup><supportedRuntime version="v4.0" sku=".NETFramework,Version=v4.8" /></startup>
              <system.serviceModel>
                <bindings>
                  <basicHttpBinding><binding name="BasicHttpBinding_IOrderService" /></basicHttpBinding>
                </bindings>
                <client>
                  <endpoint address="http://localhost:8733/OrderService/"
                            binding="basicHttpBinding"
                            bindingConfiguration="BasicHttpBinding_IOrderService"
                            contract="OrderServiceReference.IOrderService"
                            name="BasicHttpBinding_IOrderService" />
                  <endpoint address="net.tcp://orders:9000/OrderService" binding="netTcpBinding"
                            contract="OrderServiceReference.IOrderService" name="NetTcpBinding_IOrderService" />
                </client>
              </system.serviceModel>
            </configuration>
            """;

        var endpoints = ServiceModelConfigReader.ReadEndpoints(config);

        Assert.Equal(
            [
                new ServiceModelEndpoint(EndpointSource.Client, null, "BasicHttpBinding_IOrderService",
                    "http://localhost:8733/OrderService/", "basicHttpBinding", "OrderServiceReference.IOrderService"),
                new ServiceModelEndpoint(EndpointSource.Client, null, "NetTcpBinding_IOrderService",
                    "net.tcp://orders:9000/OrderService", "netTcpBinding", "OrderServiceReference.IOrderService"),
            ],
            endpoints);
    }

    [Fact]
    public void ReadEndpoints_ReadsServiceEndpointsFromWebConfig()
    {
        const string config = """
            <configuration xmlns="http://schemas.microsoft.com/.NET/Configuration/v2.0">
              <system.web><compilation debug="true" targetFramework="4.8" /></system.web>
              <system.serviceModel>
                <services>
                  <service name="Billing.InvoiceService" behaviorConfiguration="InvoiceBehavior">
                    <endpoint address="" binding="wsHttpBinding" contract="Billing.IInvoiceService" />
                    <endpoint address="mex" binding="mexHttpBinding" contract="IMetadataExchange" />
                  </service>
                  <service name="Billing.ReportService">
                    <endpoint name="reports" address="reports" binding="basicHttpBinding" contract="Billing.IReportService" />
                  </service>
                </services>
              </system.serviceModel>
            </configuration>
            """;

        var endpoints = ServiceModelConfigReader.ReadEndpoints(config);

        Assert.Equal(3, endpoints.Count);
        Assert.All(endpoints, e => Assert.Equal(EndpointSource.Service, e.Source));
        Assert.Equal(
            new ServiceModelEndpoint(EndpointSource.Service, "Billing.InvoiceService", null, "", "wsHttpBinding", "Billing.IInvoiceService"),
            endpoints[0]);
        Assert.Equal("IMetadataExchange", endpoints[1].Contract);
        Assert.Equal(
            new ServiceModelEndpoint(EndpointSource.Service, "Billing.ReportService", "reports", "reports", "basicHttpBinding", "Billing.IReportService"),
            endpoints[2]);
    }

    [Fact]
    public void ReadEndpoints_ReturnsClientThenServiceEndpointsWhenBothPresent()
    {
        const string config = """
            <configuration>
              <system.serviceModel>
                <services>
                  <service name="Gateway"><endpoint binding="basicHttpBinding" contract="IGateway" /></service>
                </services>
                <client>
                  <endpoint address="http://backend/svc" binding="basicHttpBinding" contract="IBackend" name="backend" />
                </client>
              </system.serviceModel>
            </configuration>
            """;

        var endpoints = ServiceModelConfigReader.ReadEndpoints(config);

        Assert.Equal([EndpointSource.Client, EndpointSource.Service], endpoints.Select(e => e.Source));
        Assert.Null(endpoints[1].Address);
    }

    [Theory]
    [InlineData("<configuration />")]
    [InlineData("<configuration><appSettings><add key=\"a\" value=\"b\" /></appSettings></configuration>")]
    [InlineData("<configuration><system.serviceModel><client /></system.serviceModel></configuration>")]
    [InlineData("<notConfiguration><system.serviceModel><client><endpoint contract=\"I\" /></client></system.serviceModel></notConfiguration>")]
    public void ReadEndpoints_ReturnsEmptyWhenThereAreNoEndpoints(string config)
    {
        Assert.Empty(ServiceModelConfigReader.ReadEndpoints(config));
    }

    [Fact]
    public void ReadEndpoints_ThrowsOnMalformedXml()
    {
        Assert.Throws<System.Xml.XmlException>(() => ServiceModelConfigReader.ReadEndpoints("<configuration>"));
    }
}
