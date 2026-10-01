namespace Eludite.Web.Tests;

public sealed class ServerControlScannerTests
{
    [Fact]
    public void Scan_FindsSelfClosingAspButton()
    {
        var controls = ServerControlScanner.Scan("""<asp:Button ID="SubmitButton" runat="server" Text="Submit" OnClick="SubmitButton_Click" />""");

        Assert.Equal([new ServerControl("asp", "Button", "SubmitButton")], controls);
    }

    [Fact]
    public void Scan_FindsNestedControlsInDocumentOrder()
    {
        const string markup = """
            <%@ Page Language="C#" CodeBehind="Products.aspx.cs" Inherits="Shop.Products" %>
            <%@ Register TagPrefix="uc1" TagName="Pager" Src="~/Controls/Pager.ascx" %>
            <form id="form1" runat="server">
                <asp:Panel ID="ResultsPanel" runat="server" Visible="false">
                    <asp:GridView ID="ProductsGrid" runat="server" AutoGenerateColumns="false">
                        <Columns>
                            <asp:BoundField DataField="Name" HeaderText="Name" />
                            <asp:TemplateField>
                                <ItemTemplate>
                                    <asp:Label runat="server" Text='<%# Eval("Price", "{0:C}") %>' />
                                </ItemTemplate>
                            </asp:TemplateField>
                        </Columns>
                    </asp:GridView>
                    <uc1:Pager ID="ProductsPager" runat="Server" PageSize="20" />
                </asp:Panel>
                <div id="footer" class="footer">not a server control</div>
            </form>
            """;

        var controls = ServerControlScanner.Scan(markup);

        Assert.Equal(
            [
                new ServerControl(null, "form", "form1"),
                new ServerControl("asp", "Panel", "ResultsPanel"),
                new ServerControl("asp", "GridView", "ProductsGrid"),
                new ServerControl("uc1", "Pager", "ProductsPager"),
            ],
            controls);
    }

    [Fact]
    public void Scan_SkipsControlsWithoutIds()
    {
        const string markup = """
            <asp:Literal runat="server" Text="Hello" />
            <asp:Label runat="server" ID="" />
            <script runat="server">void Page_Load() { }</script>
            <asp:HyperLink id="HomeLink" runat="server" NavigateUrl="~/" />
            """;

        var controls = ServerControlScanner.Scan(markup);

        Assert.Equal([new ServerControl("asp", "HyperLink", "HomeLink")], controls);
    }

    [Fact]
    public void Scan_HandlesMasterPagePlaceholdersAndAttributesContainingAngleBrackets()
    {
        const string markup = """
            <%@ Master Language="C#" CodeBehind="Site.master.cs" Inherits="Shop.SiteMaster" %>
            <head runat="server">
                <asp:ContentPlaceHolder ID="HeadContent" runat="server"></asp:ContentPlaceHolder>
            </head>
            <body>
                <asp:HyperLink ID="CartLink" runat="server" ToolTip="a > b" NavigateUrl='<%# GetCartUrl("x") %>' />
                <%-- <asp:Button ID="OldButton" runat="server" /> --%>
                <asp:ContentPlaceHolder ID="MainContent" runat="server" />
            </body>
            """;

        var controls = ServerControlScanner.Scan(markup);

        Assert.Equal(
            [
                new ServerControl("asp", "ContentPlaceHolder", "HeadContent"),
                new ServerControl("asp", "HyperLink", "CartLink"),
                new ServerControl("asp", "ContentPlaceHolder", "MainContent"),
            ],
            controls);
    }

    [Fact]
    public void Scan_IgnoresRunatValuesOtherThanServer()
    {
        Assert.Empty(ServerControlScanner.Scan("""<div ID="x" runat="client"></div>"""));
    }
}
