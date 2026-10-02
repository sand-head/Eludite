namespace Niello.Web.Tests;

public sealed class DesignerPartialGeneratorTests
{
    private sealed class FakeCatalog(params string[] types) : IControlTypeCatalog
    {
        private readonly Dictionary<string, string> _types = types.ToDictionary(t => t, t => t, StringComparer.OrdinalIgnoreCase);

        public string? Find(string fullName) => _types.GetValueOrDefault(fullName);
    }

    private static readonly FakeCatalog SystemWeb = new(
        "System.Web.UI.WebControls.Button",
        "System.Web.UI.WebControls.Label",
        "System.Web.UI.WebControls.TextBox",
        "System.Web.UI.WebControls.Repeater",
        "System.Web.UI.WebControls.ContentPlaceHolder",
        "System.Web.UI.UpdatePanel",
        "System.Web.UI.ScriptManager",
        "Contoso.Controls.Rating");

    [Fact]
    public void Register_ParsesNamespaceAndUserControlRegistrations()
    {
        const string markup = """
            <%@ Page Language="C#" Inherits="Shop.Products" %>
            <%@ Register TagPrefix="ajax" Namespace="Contoso.Ajax" Assembly="Contoso.Ajax" %>
            <%-- <%@ Register TagPrefix="old" Namespace="Gone" %> --%>
            <%@ Register Src="~/Controls/Pager.ascx" TagPrefix="uc1" TagName="Pager" %>
            """;

        var registrations = RegisterDirectiveParser.Parse(markup);

        Assert.Equal(
            [
                new ControlRegistration("ajax", "Contoso.Ajax", "Contoso.Ajax", null, null),
                new ControlRegistration("uc1", null, null, "Pager", "~/Controls/Pager.ascx"),
            ],
            registrations);
        Assert.True(registrations[1].IsUserControl);
    }

    [Fact]
    public void WebConfig_ReadsPagesControlsIncludingLocation()
    {
        const string config = """
            <?xml version="1.0"?>
            <configuration>
              <system.web>
                <pages>
                  <controls>
                    <add tagPrefix="contoso" namespace="Contoso.Controls" assembly="Contoso" />
                    <add tagPrefix="uc" tagName="Footer" src="~/Footer.ascx" />
                    <add namespace="NoPrefix" />
                  </controls>
                </pages>
              </system.web>
              <location path="admin">
                <system.web><pages><controls><add tagPrefix="adm" namespace="Admin.Controls" /></controls></pages></system.web>
              </location>
            </configuration>
            """;

        var registrations = WebConfigControls.Read(config);

        Assert.Equal(
            [
                new ControlRegistration("contoso", "Contoso.Controls", "Contoso", null, null),
                new ControlRegistration("uc", null, null, "Footer", "~/Footer.ascx"),
                new ControlRegistration("adm", "Admin.Controls", null, null, null),
            ],
            registrations);
    }

    [Fact]
    public void WebConfig_MalformedXmlYieldsNothing() => Assert.Empty(WebConfigControls.Read("<configuration><system.web>"));

    [Fact]
    public void Scanner_SkipsTemplatesAndContentWrappersButKeepsContentTemplate()
    {
        const string markup = """
            <asp:Content ID="BodyContent" ContentPlaceHolderID="MainContent" runat="server">
              <asp:Repeater ID="Items" runat="server">
                <ItemTemplate><asp:Label ID="ItemLabel" runat="server" /></ItemTemplate>
              </asp:Repeater>
              <asp:UpdatePanel ID="Panel" runat="server">
                <ContentTemplate><asp:TextBox ID="Query" runat="server" /></ContentTemplate>
              </asp:UpdatePanel>
              <input id="Hidden" type="hidden" runat="server">
              <% if (x < y) { %><asp:Button ID="Go" runat="server" /><% } %>
            </asp:Content>
            """;

        var ids = DesignerControlScanner.Scan(markup).Select(c => c.Id).ToList();

        Assert.Equal(["Items", "Panel", "Query", "Hidden", "Go"], ids);
    }

    [Fact]
    public void Resolver_UsesPageThenWebConfigThenFrameworkDefaults()
    {
        var resolver = new ControlTypeResolver(
            SystemWeb,
            [new ControlRegistration("ct", "Contoso.Controls", "Contoso", null, null), new ControlRegistration("uc", null, null, "Pager", "~/Pager.ascx")],
            src => src == "~/Pager.ascx" ? "Shop.Controls.Pager" : null);

        Assert.Equal("System.Web.UI.WebControls.Button", resolver.Resolve(Control("asp", "button")));
        Assert.Equal("System.Web.UI.UpdatePanel", resolver.Resolve(Control("asp", "UpdatePanel")));
        Assert.Equal("Contoso.Controls.Rating", resolver.Resolve(Control("ct", "Rating")));
        Assert.Equal("Shop.Controls.Pager", resolver.Resolve(Control("uc", "Pager")));
        Assert.Null(resolver.Resolve(Control("asp", "NoSuchControl")));
        Assert.Equal("System.Web.UI.HtmlControls.HtmlForm", resolver.Resolve(Control(null, "form")));
        Assert.Equal("System.Web.UI.HtmlControls.HtmlInputHidden", resolver.Resolve(new DesignerControl(null, "input", "h", new Dictionary<string, string> { ["type"] = "hidden" })));
        Assert.Equal("System.Web.UI.HtmlControls.HtmlGenericControl", resolver.Resolve(Control(null, "div")));
    }

    [Fact]
    public void Generator_BuildsAndRendersPartialFromMarkupAndWebConfigRegistration()
    {
        const string markup = """
            <%@ Page Language="C#" CodeBehind="Login.aspx.cs" Inherits="Shop.Account.Login, Shop" %>
            <form id="form1" runat="server">
              <asp:Button ID="LogInButton" runat="server" Text="Log in" />
              <ct:Rating ID="Stars" runat="server" />
              <ct:Missing ID="Ghost" runat="server" />
              <asp:Label ID="LogInButton" runat="server" />
            </form>
            """;
        var resolver = new ControlTypeResolver(SystemWeb, WebConfigControls.Read("""
            <configuration><system.web><pages><controls>
              <add tagPrefix="ct" namespace="Contoso.Controls" assembly="Contoso" />
            </controls></pages></system.web></configuration>
            """));

        var partial = DesignerPartialGenerator.Build(markup, resolver);

        Assert.NotNull(partial);
        Assert.Equal("Shop.Account", partial.Namespace);
        Assert.Equal("Login", partial.ClassName);
        Assert.Equal(
            [
                new DesignerField("form1", "System.Web.UI.HtmlControls.HtmlForm"),
                new DesignerField("LogInButton", "System.Web.UI.WebControls.Button"),
                new DesignerField("Stars", "Contoso.Controls.Rating"),
                new DesignerField("Ghost", DesignerPartialGenerator.FallbackType),
            ],
            partial.Fields);
        Assert.Equal("Ghost", Assert.Single(partial.Unresolved).Id);

        var source = DesignerPartialGenerator.Render(partial, "Account/Login.aspx");
        Assert.Contains("namespace Shop.Account", source, StringComparison.Ordinal);
        Assert.Contains("public partial class Login", source, StringComparison.Ordinal);
        Assert.Contains("protected global::System.Web.UI.WebControls.Button LogInButton;", source, StringComparison.Ordinal);
        Assert.Contains("protected global::Contoso.Controls.Rating Stars;", source, StringComparison.Ordinal);
    }

    [Fact]
    public void Resolver_TrustsRegistrationForAssemblyTheCatalogNeverSaw()
    {
        var catalog = new MetadataTypeCatalog([typeof(ServerControl).Assembly.Location]);
        var resolver = new ControlTypeResolver(catalog, [
            new ControlRegistration("cc1", "umbraco.uicontrols", "controls", null, null),
            new ControlRegistration("nw", "Niello.Web", "Niello.Web", null, null),
        ]);

        // "controls" is a project reference that is not built: unverifiable, so trusted.
        Assert.Equal("umbraco.uicontrols.Pane", resolver.Resolve(Control("cc1", "Pane")));
        // Niello.Web is indexed: verified match, and a miss stays a miss.
        Assert.Equal("Niello.Web.ServerControl", resolver.Resolve(Control("nw", "servercontrol")));
        Assert.Null(resolver.Resolve(Control("nw", "NoSuchType")));
    }

    [Fact]
    public void Generator_ReturnsNullWithoutInherits() =>
        Assert.Null(DesignerPartialGenerator.Build("""<%@ Page Language="C#" %><asp:Button ID="B" runat="server" />""", new ControlTypeResolver(null, [])));

    [Fact]
    public void MetadataCatalog_IndexesPublicTypesOfRealAssembly()
    {
        var catalog = new MetadataTypeCatalog([typeof(ServerControl).Assembly.Location, "/nonexistent/assembly.dll"]);

        Assert.Equal("Niello.Web.ServerControl", catalog.Find("niello.web.servercontrol"));
        Assert.Null(catalog.Find("Niello.Web.AspxMarkup")); // internal
        Assert.Single(catalog.Skipped);
    }

    private static DesignerControl Control(string? prefix, string tag) => new(prefix, tag, "x", new Dictionary<string, string>());
}
