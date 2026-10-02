namespace Eludite.Web.Tests;

public sealed class AspxDirectiveParserTests
{
    [Fact]
    public void Parse_PageWithCodeBehind()
    {
        const string markup = """
            <%@ Page Title="Contact" Language="C#" MasterPageFile="~/Site.Master" AutoEventWireup="true"
                CodeBehind="Contact.aspx.cs" Inherits="WingtipToys.Contact" %>
            <%@ Register TagPrefix="uc1" TagName="Address" Src="~/Controls/Address.ascx" %>

            <asp:Content ID="BodyContent" ContentPlaceHolderID="MainContent" runat="server">
                <h2><%: Title %>.</h2>
            </asp:Content>
            """;

        var directive = AspxDirectiveParser.Parse(markup);

        Assert.NotNull(directive);
        Assert.Equal(DirectiveKind.Page, directive.Kind);
        Assert.Equal("C#", directive.Language);
        Assert.Equal("Contact.aspx.cs", directive.CodeBehind);
        Assert.Null(directive.CodeFile);
        Assert.Equal("WingtipToys.Contact", directive.Inherits);
        Assert.True(directive.AutoEventWireup);
        Assert.Equal(2, directive.OtherAttributes.Count);
        Assert.Equal("Contact", directive.OtherAttributes["Title"]);
        Assert.Equal("~/Site.Master", directive.OtherAttributes["masterpagefile"]);
    }

    [Fact]
    public void Parse_MasterPage()
    {
        const string markup = """
            <%@ Master Language="C#" AutoEventWireup="true" CodeBehind="Site.master.cs" Inherits="WingtipToys.SiteMaster" %>

            <!DOCTYPE html>
            <html lang="en">
            <head runat="server"><title><%: Page.Title %></title></head>
            """;

        var directive = AspxDirectiveParser.Parse(markup);

        Assert.NotNull(directive);
        Assert.Equal(DirectiveKind.Master, directive.Kind);
        Assert.Equal("Site.master.cs", directive.CodeBehind);
        Assert.Equal("WingtipToys.SiteMaster", directive.Inherits);
        Assert.Empty(directive.OtherAttributes);
    }

    [Fact]
    public void Parse_UserControlWithCodeFileAndVb()
    {
        const string markup = "<%@ control language='VB' autoeventwireup=false codefile=\"Header.ascx.vb\" inherits=\"Header\" ClassName=\"HeaderControl\" %>";

        var directive = AspxDirectiveParser.Parse(markup);

        Assert.NotNull(directive);
        Assert.Equal(DirectiveKind.Control, directive.Kind);
        Assert.Equal("VB", directive.Language);
        Assert.Null(directive.CodeBehind);
        Assert.Equal("Header.ascx.vb", directive.CodeFile);
        Assert.False(directive.AutoEventWireup);
        Assert.Equal("HeaderControl", Assert.Single(directive.OtherAttributes).Value);
    }

    [Fact]
    public void Parse_SkipsLeadingRegisterDirectivesAndServerComments()
    {
        const string markup = """
            <%-- <%@ Page Language="VB" Inherits="Old" %> --%>
            <%@ Register Assembly="AjaxControlToolkit" Namespace="AjaxControlToolkit" TagPrefix="ajax" %>
            <%@Page Language="C#" Inherits="Shop.Cart"%>
            """;

        var directive = AspxDirectiveParser.Parse(markup);

        Assert.NotNull(directive);
        Assert.Equal("C#", directive.Language);
        Assert.Equal("Shop.Cart", directive.Inherits);
        Assert.Null(directive.AutoEventWireup);
    }

    [Theory]
    [InlineData("")]
    [InlineData("<html><body>plain html</body></html>")]
    [InlineData("<%@ Import Namespace=\"System.Data\" %>")]
    [InlineData("<%@ WebHandler Language=\"C#\" Class=\"Handler\" %>")]
    public void Parse_ReturnsNullWithoutMainDirective(string markup)
    {
        Assert.Null(AspxDirectiveParser.Parse(markup));
    }
}
